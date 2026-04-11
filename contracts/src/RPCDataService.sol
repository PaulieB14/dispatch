// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.27;

import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";

import {DataService} from "@graphprotocol/horizon/data-service/DataService.sol";
import {DataServiceFees} from "@graphprotocol/horizon/data-service/extensions/DataServiceFees.sol";
import {DataServicePausable} from "@graphprotocol/horizon/data-service/extensions/DataServicePausable.sol";
import {IGraphPayments} from "@graphprotocol/horizon/interfaces/IGraphPayments.sol";
import {IGraphTallyCollector} from "@graphprotocol/horizon/interfaces/IGraphTallyCollector.sol";
import {IHorizonStaking} from "@graphprotocol/horizon/interfaces/IHorizonStaking.sol";

import {IRPCDataService} from "./interfaces/IRPCDataService.sol";
import {StateProofVerifier} from "./lib/StateProofVerifier.sol";

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
    using SafeERC20 for IERC20;

    // -------------------------------------------------------------------------
    // Constants
    // -------------------------------------------------------------------------

    /// @notice Default minimum GRT provision per chain.
    /// Lower than SubgraphService (100k) — RPC proxying has lower infrastructure
    /// complexity (no Graph Node, no subgraph compilation).
    uint256 public constant DEFAULT_MIN_PROVISION = 25_000e18;

    /// @notice Absolute lower bound on the thawing period. The governance-adjustable
    /// `minThawingPeriod` cannot be set below this value.
    uint64 public constant MIN_THAWING_PERIOD = 14 days;

    /// @notice Stake locked per GRT of fees collected. Matches SubgraphService.
    uint256 public constant STAKE_TO_FEES_RATIO = 5;

    /// @notice GRT slashed per successful Tier 1 fraud proof.
    /// Capped by the provider's actual provision if it is smaller.
    uint256 public constant SLASH_AMOUNT = 10_000e18;

    /// @notice Fraction of slashed tokens awarded to the challenger as a bounty (PPM, 50%).
    uint256 public constant CHALLENGER_REWARD_PPM = 500_000;

    /// @notice GRT bond required to propose a new chain permissionlessly.
    uint256 public constant CHAIN_BOND_AMOUNT = 100_000e18;

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

    /// @notice GRT token — used for permissionless chain proposal bonds.
    IERC20 private immutable GRT;

    /// @notice Trusted state roots: blockHash → stateRoot.
    /// @dev Populated by governance/oracle after verifying an Ethereum block header.
    ///      Required because Arbitrum contracts cannot read L1 block hashes natively.
    mapping(bytes32 => bytes32) public trustedStateRoots;

    /// @notice Pending permissionless chain proposals.
    mapping(uint256 => ChainBond) public pendingChainBonds;

    /// @notice GRT issuance rate per compute unit. Zero = issuance disabled.
    uint256 public issuancePerCU;

    /// @notice Governance-adjustable thawing period (lower-bounded by MIN_THAWING_PERIOD).
    uint64 public minThawingPeriod;

    /// @notice GRT deposited by governance to fund provider rewards.
    uint256 public rewardsPool;

    /// @notice Accrued but unclaimed GRT rewards per recipient.
    mapping(address => uint256) public pendingRewards;

    event TrustedStateRootSet(bytes32 indexed blockHash, bytes32 stateRoot);

    // -------------------------------------------------------------------------
    // Constructor
    // -------------------------------------------------------------------------

    /// @param owner_ Initial owner (governance multisig).
    /// @param controller The Graph Protocol controller address (GraphDirectory).
    /// @param graphTallyCollector Address of the deployed GraphTallyCollector.
    /// @param pauseGuardian Address authorised to pause the service in an emergency.
    /// @param grtToken_ GRT ERC-20 token address (used for chain proposal bonds).
    constructor(
        address owner_,
        address controller,
        address graphTallyCollector,
        address pauseGuardian,
        address grtToken_
    ) Ownable(owner_) DataService(controller) {
        GRAPH_TALLY_COLLECTOR = IGraphTallyCollector(graphTallyCollector);
        GRT = IERC20(grtToken_);
        minThawingPeriod = MIN_THAWING_PERIOD;
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
            enabled: true, minProvisionTokens: minProvisionTokens == 0 ? DEFAULT_MIN_PROVISION : minProvisionTokens
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
    function setMinThawingPeriod(uint64 period) external onlyOwner {
        if (period < MIN_THAWING_PERIOD) revert ThawingPeriodTooShort(MIN_THAWING_PERIOD, period);
        minThawingPeriod = period;
        emit MinThawingPeriodSet(period);
    }

    /// @inheritdoc IRPCDataService
    function setTrustedStateRoot(bytes32 blockHash, bytes32 stateRoot) external onlyOwner {
        trustedStateRoots[blockHash] = stateRoot;
        emit TrustedStateRootSet(blockHash, stateRoot);
    }

    // -------------------------------------------------------------------------
    // Permissionless chain proposals
    // -------------------------------------------------------------------------

    /// @inheritdoc IRPCDataService
    function proposeChain(uint256 chainId) external whenNotPaused {
        if (supportedChains[chainId].enabled) revert ChainAlreadySupported(chainId);
        if (pendingChainBonds[chainId].proposer != address(0)) revert ChainAlreadyProposed(chainId);
        GRT.safeTransferFrom(msg.sender, address(this), CHAIN_BOND_AMOUNT);
        pendingChainBonds[chainId] =
            ChainBond({proposer: msg.sender, amount: CHAIN_BOND_AMOUNT, proposedAt: block.timestamp});
        emit ChainProposed(chainId, msg.sender, CHAIN_BOND_AMOUNT);
    }

    /// @inheritdoc IRPCDataService
    function approveProposedChain(uint256 chainId, uint256 minProvisionTokens) external onlyOwner {
        ChainBond memory bond = pendingChainBonds[chainId];
        if (bond.proposer == address(0)) revert ChainNotProposed(chainId);
        delete pendingChainBonds[chainId];
        uint256 min = minProvisionTokens == 0 ? DEFAULT_MIN_PROVISION : minProvisionTokens;
        supportedChains[chainId] = ChainConfig({enabled: true, minProvisionTokens: min});
        GRT.safeTransfer(bond.proposer, bond.amount);
        emit ChainAdded(chainId, min);
        emit ChainBondReleased(chainId, bond.proposer, bond.amount);
    }

    /// @inheritdoc IRPCDataService
    function rejectProposedChain(uint256 chainId) external onlyOwner {
        ChainBond memory bond = pendingChainBonds[chainId];
        if (bond.proposer == address(0)) revert ChainNotProposed(chainId);
        delete pendingChainBonds[chainId];
        GRT.safeTransfer(owner(), bond.amount);
        emit ChainBondForfeited(chainId, bond.amount);
    }

    /// @inheritdoc IRPCDataService
    function setIssuancePerCU(uint256 rate) external onlyOwner {
        issuancePerCU = rate;
        emit IssuanceRateSet(rate);
    }

    /// @inheritdoc IRPCDataService
    function depositRewardsPool(uint256 amount) external onlyOwner {
        GRT.safeTransferFrom(msg.sender, address(this), amount);
        rewardsPool += amount;
        emit RewardsDeposited(amount);
    }

    /// @inheritdoc IRPCDataService
    function withdrawRewardsPool(uint256 amount) external onlyOwner {
        if (amount > rewardsPool) revert InsufficientRewardsPool(rewardsPool, amount);
        rewardsPool -= amount;
        GRT.safeTransfer(msg.sender, amount);
        emit RewardsWithdrawn(amount);
    }

    // -------------------------------------------------------------------------
    // Provider rewards
    // -------------------------------------------------------------------------

    /// @inheritdoc IRPCDataService
    function claimRewards() external {
        uint256 amount = pendingRewards[msg.sender];
        if (amount == 0) revert NoPendingRewards(msg.sender);
        pendingRewards[msg.sender] = 0;
        GRT.safeTransfer(msg.sender, amount);
        emit RewardsClaimed(msg.sender, amount);
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

        (string memory endpoint, string memory geoHash, address dest) = abi.decode(data, (string, string, address));
        registeredProviders[serviceProvider] = true;
        paymentsDestination[serviceProvider] = dest == address(0) ? serviceProvider : dest;

        emit ProviderRegistered(serviceProvider, endpoint, geoHash);
    }

    /// @notice Deregister as an RPC provider.
    /// @dev All chain registrations must be stopped first.
    function deregister(address serviceProvider, bytes calldata) external onlyAuthorizedForProvision(serviceProvider) {
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

        (uint64 chainId, uint8 tier, string memory endpoint) = abi.decode(data, (uint64, uint8, string));

        ChainConfig storage cfg = supportedChains[chainId];
        if (!cfg.enabled) revert ChainNotSupported(chainId);

        // Validate provision meets the per-chain minimum.
        IHorizonStaking.Provision memory provision = _graphStaking().getProvision(serviceProvider, address(this));
        if (provision.tokens < cfg.minProvisionTokens) {
            revert InsufficientProvision(cfg.minProvisionTokens, provision.tokens);
        }

        _providerChains[serviceProvider].push(
            ChainRegistration({chainId: chainId, tier: CapabilityTier(tier), endpoint: endpoint, active: true})
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
    function collect(address serviceProvider, IGraphPayments.PaymentTypes paymentType, bytes calldata data)
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

        require(signedRav.rav.serviceProvider == serviceProvider, "RPCDataService: RAV provider mismatch");

        // Release any expired stake claims before locking new ones.
        _releaseStake(serviceProvider, 0);

        // Collect via GraphTallyCollector → PaymentsEscrow → GraphPayments.
        // The RAV's dataService field must equal address(this) — enforced by GraphTallyCollector.
        // Fees flow to paymentsDestination[serviceProvider], not necessarily serviceProvider itself.
        fees = GRAPH_TALLY_COLLECTOR.collect(
            paymentType,
            abi.encode(
                signedRav,
                uint256(0), // dataServiceCut=0 for Phase 1 (no curation)
                paymentsDestination[serviceProvider] // receiverDestination: where GRT lands
            ),
            tokensToCollect
        );

        if (fees > 0) {
            // Lock stake proportional to fees — released after the dispute window.
            _lockStake(serviceProvider, fees * STAKE_TO_FEES_RATIO, block.timestamp + minThawingPeriod);

            // Accrue issuance reward if the pool has funds.
            if (issuancePerCU > 0 && rewardsPool > 0) {
                uint256 reward = fees * issuancePerCU / 1e18;
                if (reward > rewardsPool) reward = rewardsPool;
                rewardsPool -= reward;
                address dest = paymentsDestination[serviceProvider];
                pendingRewards[dest] += reward;
                emit RewardsAccrued(dest, reward);
            }
        }
    }

    /// @notice Submit a Tier 1 fraud proof to slash a provider.
    ///
    /// The challenger supplies an EIP-1186 Merkle proof showing that the state value
    /// at the given block hash differs from what the provider claimed to serve.
    /// The block's state root must have been registered by governance via
    /// `setTrustedStateRoot` before the call.
    ///
    /// On success: `SLASH_AMOUNT` GRT is removed from the provider's provision;
    /// 50% is awarded to the challenger and the remainder goes to the protocol treasury.
    ///
    /// @param serviceProvider The provider to slash.
    /// @param data            ABI-encoded `Tier1FraudProof`.
    function slash(address serviceProvider, bytes calldata data) external override whenNotPaused {
        if (!registeredProviders[serviceProvider]) revert ProviderNotRegistered(serviceProvider);

        Tier1FraudProof memory proof = abi.decode(data, (Tier1FraudProof));

        bytes32 stateRoot = trustedStateRoots[proof.blockHash];
        if (stateRoot == bytes32(0)) revert UntrustedBlockHash(proof.blockHash);

        // Verify account proof and decode state.
        StateProofVerifier.Account memory acc =
            StateProofVerifier.verifyAccount(stateRoot, proof.account, proof.accountProof);

        // Resolve actual on-chain value for the disputed field.
        uint256 actualValue;
        if (proof.disputeType == DisputeType.Balance) {
            actualValue = acc.balance;
        } else if (proof.disputeType == DisputeType.Nonce) {
            actualValue = acc.nonce;
        } else {
            // Storage dispute — derive value from the account's storageRoot.
            bytes32 storageValue =
                StateProofVerifier.verifyStorage(acc.storageRoot, proof.storageSlot, proof.storageProof);
            actualValue = uint256(storageValue);
        }

        // Revert if the proof shows the provider was correct.
        if (actualValue == proof.claimedValue) {
            revert InvalidFraudProof("claimed value matches on-chain state");
        }

        // Compute slash amount — capped by the provider's actual provision.
        IHorizonStaking.Provision memory provision = _graphStaking().getProvision(serviceProvider, address(this));
        uint256 tokens = provision.tokens < SLASH_AMOUNT ? provision.tokens : SLASH_AMOUNT;
        uint256 tokensVerifier = tokens * CHALLENGER_REWARD_PPM / 1_000_000;

        _graphStaking().slash(serviceProvider, tokens, tokensVerifier, proof.challenger);

        emit FraudProofSubmitted(serviceProvider, proof.challenger, tokens);
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
    function getChainRegistrations(address provider) external view override returns (ChainRegistration[] memory) {
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
