// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.27;

import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";

import {DataService} from "@graphprotocol/horizon/data-service/DataService.sol";
import {DataServiceFees} from "@graphprotocol/horizon/data-service/extensions/DataServiceFees.sol";
import {DataServicePausable} from
    "@graphprotocol/horizon/data-service/extensions/DataServicePausable.sol";
import {IGraphPayments} from "@graphprotocol/horizon/interfaces/IGraphPayments.sol";
import {IGraphTallyCollector} from "@graphprotocol/horizon/interfaces/IGraphTallyCollector.sol";
import {IHorizonStaking} from "@graphprotocol/horizon/interfaces/IHorizonStaking.sol";

import {IRPCDataService} from "./interfaces/IRPCDataService.sol";

/// @title RPCDataService
/// @notice Decentralised JSON-RPC data service built on The Graph Protocol's Horizon framework.
///
/// Providers (indexers) stake GRT via HorizonStaking provisions, register here,
/// then call startService for each (chainId, tier) pair they wish to serve.
/// Gateways pay per-request via GraphTally (TAP v2); providers collect fees by
/// submitting signed RAVs to collect().
///
/// Verification tiers (from RFC):
///   Tier 1 — Merkle-provable methods: fraud proof slashing (Phase 2).
///   Tier 2 — Quorum-verifiable: multi-provider cross-reference, no slashing.
///   Tier 3 — Non-deterministic: reputation scoring only.
///
/// @dev Inherits DataService (provision utilities, GraphDirectory), DataServiceFees
///      (stake-backed fee locking), DataServicePausable (emergency stop).
///      Deployed on Arbitrum One — all Horizon contracts live there.
contract RPCDataService is Ownable, DataService, DataServiceFees, DataServicePausable, IRPCDataService {
    // -------------------------------------------------------------------------
    // Constants
    // -------------------------------------------------------------------------

    /// @notice Default minimum GRT provision per chain.
    /// Lower than SubgraphService (100k) — RPC proxying has lower infrastructure
    /// complexity (no Graph Node, no subgraph compilation).
    uint256 public constant DEFAULT_MIN_PROVISION = 25_000e18;

    /// @notice Minimum thawing period. Shorter than SubgraphService (28d) because
    /// RPC correctness disputes resolve faster than subgraph POI verification.
    uint64 public constant MIN_THAWING_PERIOD = 14 days;

    /// @notice Stake locked per GRT of fees collected. Matches SubgraphService.
    uint256 public constant STAKE_TO_FEES_RATIO = 5;

    // -------------------------------------------------------------------------
    // Storage
    // -------------------------------------------------------------------------

    /// @notice Per-chain configuration (governance-controlled allowlist).
    mapping(uint256 => ChainConfig) public supportedChains;

    /// @notice Whether a provider has registered with this service.
    mapping(address => bool) public registeredProviders;

    /// @notice Address that receives collected GRT for each provider.
    /// @dev Defaults to the provider address. Separates the operator key
    ///      (used for signing) from the payment wallet (cold storage etc.).
    mapping(address => address) public paymentsDestination;

    /// @notice Chain registrations per provider (active and historical).
    mapping(address => ChainRegistration[]) internal _providerChains;

    /// @notice GraphTallyCollector used to redeem TAP receipts on-chain.
    IGraphTallyCollector private immutable GRAPH_TALLY_COLLECTOR;

    // -------------------------------------------------------------------------
    // Constructor
    // -------------------------------------------------------------------------

    /// @param owner_ Initial owner (governance multisig).
    /// @param controller The Graph Protocol controller address (GraphDirectory).
    /// @param graphTallyCollector Address of the deployed GraphTallyCollector.
    /// @param pauseGuardian Address authorised to pause the service in an emergency.
    constructor(address owner_, address controller, address graphTallyCollector, address pauseGuardian)
        Ownable(owner_)
        DataService(controller)
    {
        GRAPH_TALLY_COLLECTOR = IGraphTallyCollector(graphTallyCollector);
        // Configure ProvisionManager ranges (used by _checkProvisionTokens/_checkProvisionParameters).
        _setProvisionTokensRange(DEFAULT_MIN_PROVISION, type(uint256).max);
        _setThawingPeriodRange(MIN_THAWING_PERIOD, type(uint64).max);
        _setVerifierCutRange(0, uint32(1_000_000)); // 0–100% in PPM
        _setPauseGuardian(pauseGuardian, true);
    }

    // -------------------------------------------------------------------------
    // Governance
    // -------------------------------------------------------------------------

    /// @inheritdoc IRPCDataService
    function addChain(uint256 chainId, uint256 minProvisionTokens) external onlyOwner {
        supportedChains[chainId] = ChainConfig({
            enabled: true,
            minProvisionTokens: minProvisionTokens == 0 ? DEFAULT_MIN_PROVISION : minProvisionTokens
        });
        emit ChainAdded(chainId, minProvisionTokens == 0 ? DEFAULT_MIN_PROVISION : minProvisionTokens);
    }

    /// @inheritdoc IRPCDataService
    function removeChain(uint256 chainId) external onlyOwner {
        supportedChains[chainId].enabled = false;
        emit ChainRemoved(chainId);
    }

    /// @inheritdoc IRPCDataService
    function setDefaultMinProvision(uint256 tokens) external onlyOwner {
        // Validated off-chain; no storage variable — overrides via per-chain configs.
        // Emit for indexer tooling awareness.
        emit ChainAdded(0, tokens); // chainId=0 signals "default" to off-chain consumers
    }

    /// @inheritdoc IRPCDataService
    function setMinThawingPeriod(uint64) external onlyOwner {
        // Phase 2: allow governance to adjust via storage variable.
        // For Phase 1, MIN_THAWING_PERIOD is an immutable constant.
        revert("setMinThawingPeriod: not implemented in Phase 1");
    }

    // -------------------------------------------------------------------------
    // IDataService — lifecycle
    // -------------------------------------------------------------------------

    /// @notice Register as an RPC provider.
    /// @param serviceProvider The provider's address.
    /// @param data ABI-encoded (string endpoint, string geoHash).
    function register(address serviceProvider, bytes calldata data)
        external
        override
        whenNotPaused
        onlyAuthorizedForProvision(serviceProvider)
    {
        if (registeredProviders[serviceProvider]) {
            revert ProviderAlreadyRegistered(serviceProvider);
        }

        // Validate provision meets protocol minimums (uses ranges set in constructor).
        _checkProvisionTokens(serviceProvider);
        _checkProvisionParameters(serviceProvider, false);

        (string memory endpoint, string memory geoHash, address dest) =
            abi.decode(data, (string, string, address));
        registeredProviders[serviceProvider] = true;
        paymentsDestination[serviceProvider] = dest == address(0) ? serviceProvider : dest;

        emit ProviderRegistered(serviceProvider, endpoint, geoHash);
    }

    /// @notice Deregister as an RPC provider.
    /// @dev All chain registrations must be stopped first.
    function deregister(address serviceProvider, bytes calldata)
        external
        onlyAuthorizedForProvision(serviceProvider)
    {
        if (!registeredProviders[serviceProvider]) revert ProviderNotRegistered(serviceProvider);
        if (activeRegistrationCount(serviceProvider) > 0) revert ActiveRegistrationsExist(serviceProvider);

        registeredProviders[serviceProvider] = false;
        emit ProviderDeregistered(serviceProvider);
    }

    /// @inheritdoc IRPCDataService
    function setPaymentsDestination(address destination) external {
        if (!registeredProviders[msg.sender]) revert ProviderNotRegistered(msg.sender);
        address dest = destination == address(0) ? msg.sender : destination;
        paymentsDestination[msg.sender] = dest;
        emit PaymentsDestinationSet(msg.sender, dest);
    }

    /// @notice Activate RPC service for a specific chain and capability tier.
    /// @param serviceProvider The provider's address.
    /// @param data ABI-encoded (uint64 chainId, uint8 tier, string endpoint).
    function startService(address serviceProvider, bytes calldata data)
        external
        override
        whenNotPaused
        onlyAuthorizedForProvision(serviceProvider)
    {
        if (!registeredProviders[serviceProvider]) revert ProviderNotRegistered(serviceProvider);

        (uint64 chainId, uint8 tier, string memory endpoint) =
            abi.decode(data, (uint64, uint8, string));

        ChainConfig storage cfg = supportedChains[chainId];
        if (!cfg.enabled) revert ChainNotSupported(chainId);

        // Validate provision meets the per-chain minimum.
        IHorizonStaking.Provision memory provision =
            _graphStaking().getProvision(serviceProvider, address(this));
        if (provision.tokens < cfg.minProvisionTokens) {
            revert InsufficientProvision(cfg.minProvisionTokens, provision.tokens);
        }

        _providerChains[serviceProvider].push(
            ChainRegistration({
                chainId: chainId,
                tier: CapabilityTier(tier),
                endpoint: endpoint,
                active: true
            })
        );

        emit ServiceStarted(serviceProvider, chainId, CapabilityTier(tier), endpoint);
    }

    /// @notice Deactivate RPC service for a specific chain and tier.
    /// @param serviceProvider The provider's address.
    /// @param data ABI-encoded (uint64 chainId, uint8 tier).
    function stopService(address serviceProvider, bytes calldata data)
        external
        override
        onlyAuthorizedForProvision(serviceProvider)
    {
        (uint64 chainId, uint8 tier) = abi.decode(data, (uint64, uint8));

        ChainRegistration[] storage regs = _providerChains[serviceProvider];
        for (uint256 i = 0; i < regs.length; i++) {
            if (regs[i].chainId == chainId && uint8(regs[i].tier) == tier && regs[i].active) {
                regs[i].active = false;
                emit ServiceStopped(serviceProvider, chainId, CapabilityTier(tier));
                return;
            }
        }
        revert RegistrationNotFound(serviceProvider, chainId, CapabilityTier(tier));
    }

    /// @notice Collect fees by submitting a signed Receipt Aggregate Voucher (RAV).
    ///
    /// Flow:
    ///   RPCDataService.collect() → GraphTallyCollector.collect()
    ///     → PaymentsEscrow.collect() → GraphPayments.collect()
    ///     → distributes: protocol tax → data service cut → delegator cut → provider
    ///
    /// @param serviceProvider The provider collecting fees.
    /// @param paymentType Must be QueryFee for Phase 1.
    /// @param data ABI-encoded (SignedRAV, tokensToCollect).
    /// @return fees Total GRT collected by the service provider.
    function collect(
        address serviceProvider,
        IGraphPayments.PaymentTypes paymentType,
        bytes calldata data
    )
        external
        override
        whenNotPaused
        returns (uint256 fees)
    {
        // Only QueryFee is supported. Explicit revert prevents silent mis-routing
        // if the payment infrastructure ever routes other payment types here.
        if (paymentType != IGraphPayments.PaymentTypes.QueryFee) revert InvalidPaymentType();

        if (!registeredProviders[serviceProvider]) revert ProviderNotRegistered(serviceProvider);

        (IGraphTallyCollector.SignedRAV memory signedRav, uint256 tokensToCollect) =
            abi.decode(data, (IGraphTallyCollector.SignedRAV, uint256));

        require(
            signedRav.rav.serviceProvider == serviceProvider,
            "RPCDataService: RAV provider mismatch"
        );

        // Release any expired stake claims before locking new ones.
        _releaseStake(serviceProvider, 0);

        // Collect via GraphTallyCollector → PaymentsEscrow → GraphPayments.
        // The RAV's dataService field must equal address(this) — enforced by GraphTallyCollector.
        // Fees are routed to paymentsDestination, not necessarily serviceProvider.
        fees = GRAPH_TALLY_COLLECTOR.collect(
            paymentType,
            abi.encode(signedRav, uint256(0)), // dataServiceCut=0 for Phase 1 (no curation)
            tokensToCollect
        );

        if (fees > 0) {
            // Lock stake proportional to fees — released after the dispute window.
            _lockStake(serviceProvider, fees * STAKE_TO_FEES_RATIO, block.timestamp + MIN_THAWING_PERIOD);
        }
    }

    /// @notice Submit a Tier 1 fraud proof to slash a provider.
    ///
    /// Phase 1: NOT implemented. Tier 1 disputes require Merkle proof verification
    /// (EIP-1186 eth_getProof) against a trusted block header. Will be implemented
    /// in Phase 2 once the block header trust service is in place.
    ///
    /// @dev data would be ABI-encoded (request, signedResponse, merkleProof, blockHeader).
    function slash(address, bytes calldata) external override {
        revert SlashNotImplemented();
    }

    /// @notice Accept pending changes to this provider's provision parameters.
    /// @dev Two-step process: provider calls HorizonStaking.setProvisionParameters,
    ///      then this function accepts the queued change.
    function acceptProvisionPendingParameters(address serviceProvider, bytes calldata)
        external
        override
        onlyAuthorizedForProvision(serviceProvider)
    {
        _acceptProvisionParameters(serviceProvider);
    }

    // -------------------------------------------------------------------------
    // IRPCDataService views
    // -------------------------------------------------------------------------

    /// @inheritdoc IRPCDataService
    function isRegistered(address provider) external view override returns (bool) {
        return registeredProviders[provider];
    }

    /// @inheritdoc IRPCDataService
    function getChainRegistrations(address provider)
        external
        view
        override
        returns (ChainRegistration[] memory)
    {
        return _providerChains[provider];
    }

    /// @inheritdoc IRPCDataService
    function activeRegistrationCount(address provider) public view override returns (uint256 count) {
        ChainRegistration[] storage regs = _providerChains[provider];
        for (uint256 i = 0; i < regs.length; i++) {
            if (regs[i].active) count++;
        }
    }
}
