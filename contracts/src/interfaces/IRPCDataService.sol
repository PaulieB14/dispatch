// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.27;

/// @title IRPCDataService
/// @notice Interface for the dRPC data service on The Graph Protocol's Horizon framework.
///
/// A provider (indexer) lifecycle:
///   register → startService (per chain/tier) → [collect]* → stopService → deregister
///
/// Provisions are managed via HorizonStaking: the provider calls
/// HorizonStaking.provision(provider, RPCDataService, tokens, maxVerifierCut, thawingPeriod)
/// before registering here.
interface IRPCDataService {
    // -------------------------------------------------------------------------
    // Types
    // -------------------------------------------------------------------------

    /// Capability tiers determine what infrastructure and methods a provider offers.
    enum CapabilityTier {
        Standard, // 0 — full node, last 128 blocks of state
        Archive, // 1 — full historical state
        Debug, // 2 — debug_* and trace_* methods
        WebSocket // 3 — real-time eth_subscribe
    }

    struct ChainConfig {
        bool enabled;
        uint256 minProvisionTokens; // Minimum GRT provision required to serve this chain
    }

    /// @notice A pending permissionless chain proposal with its GRT bond.
    struct ChainBond {
        address proposer;
        uint256 amount;
        uint256 proposedAt;
    }

    struct ChainRegistration {
        uint64 chainId;
        CapabilityTier tier;
        string endpoint; // HTTPS or WSS URL of the provider's RPC node
        bool active;
    }

    /// @notice The on-chain field that the challenger claims was mis-reported.
    enum DisputeType {
        Balance, // provider returned wrong eth_getBalance
        Nonce, // provider returned wrong eth_getTransactionCount
        Storage // provider returned wrong eth_getStorageAt
    }

    /// @notice Tier 1 fraud proof: EIP-1186 Merkle proof showing a provider's response
    ///         contradicts the canonical on-chain state at a trusted block.
    struct Tier1FraudProof {
        address challenger;
        uint64 chainId;
        address account;
        bytes32 blockHash;
        bytes32 storageSlot; // Zero unless disputeType == Storage.
        bytes[] accountProof;
        bytes[] storageProof; // Empty unless disputeType == Storage.
        uint256 claimedValue; // The (incorrect) value the provider served.
        DisputeType disputeType;
    }

    // -------------------------------------------------------------------------
    // Events
    // -------------------------------------------------------------------------

    event ChainAdded(uint256 indexed chainId, uint256 minProvisionTokens);
    event ChainRemoved(uint256 indexed chainId);
    event ChainProposed(uint256 indexed chainId, address indexed proposer, uint256 bondAmount);
    event ChainBondReleased(uint256 indexed chainId, address indexed proposer, uint256 bondAmount);
    event ChainBondForfeited(uint256 indexed chainId, uint256 bondAmount);
    event IssuanceRateSet(uint256 issuancePerCU);
    event MinThawingPeriodSet(uint64 period);
    event RewardsAccrued(address indexed recipient, uint256 amount);
    event RewardsClaimed(address indexed recipient, uint256 amount);
    event RewardsDeposited(uint256 amount);
    event RewardsWithdrawn(uint256 amount);
    event ProviderRegistered(address indexed provider, string endpoint, string geoHash);
    event ProviderDeregistered(address indexed provider);
    event PaymentsDestinationSet(address indexed provider, address indexed destination);
    event ServiceStarted(address indexed provider, uint64 indexed chainId, CapabilityTier tier, string endpoint);
    event ServiceStopped(address indexed provider, uint64 indexed chainId, CapabilityTier tier);
    event FraudProofSubmitted(address indexed provider, address indexed challenger, uint256 slashAmount);

    // -------------------------------------------------------------------------
    // Errors
    // -------------------------------------------------------------------------

    error ChainNotSupported(uint256 chainId);
    error ChainAlreadySupported(uint256 chainId);
    error ChainAlreadyProposed(uint256 chainId);
    error ChainNotProposed(uint256 chainId);
    error ProviderAlreadyRegistered(address provider);
    error ProviderNotRegistered(address provider);
    error ActiveRegistrationsExist(address provider);
    error InsufficientProvision(uint256 required, uint256 actual);
    error ThawingPeriodTooShort(uint64 required, uint64 actual);
    error RegistrationNotFound(address provider, uint64 chainId, CapabilityTier tier);
    error InvalidFraudProof(string reason);
    error InvalidPaymentType();
    error UntrustedBlockHash(bytes32 blockHash);
    error InsufficientRewardsPool(uint256 available, uint256 required);
    error NoPendingRewards(address provider);

    // -------------------------------------------------------------------------
    // Governance (owner-only)
    // -------------------------------------------------------------------------

    /// @notice Add a chain to the supported set.
    /// @param chainId EIP-155 chain ID.
    /// @param minProvisionTokens Minimum GRT stake required. Pass 0 to use the protocol default.
    function addChain(uint256 chainId, uint256 minProvisionTokens) external;

    /// @notice Remove a chain from the supported set. Existing registrations are unaffected
    ///         until providers call stopService.
    function removeChain(uint256 chainId) external;

    /// @notice Propose adding a new chain permissionlessly by posting a GRT bond.
    /// @dev Caller must have approved CHAIN_BOND_AMOUNT GRT to this contract.
    ///      Governance then approves or rejects the proposal.
    function proposeChain(uint256 chainId) external;

    /// @notice Approve a pending chain proposal, enabling the chain and refunding the bond.
    function approveProposedChain(uint256 chainId, uint256 minProvisionTokens) external;

    /// @notice Reject a pending chain proposal, forfeiting the bond to the treasury (owner).
    function rejectProposedChain(uint256 chainId) external;

    /// @notice Update the default minimum provision tokens.
    function setDefaultMinProvision(uint256 tokens) external;

    /// @notice Update the minimum thawing period.
    function setMinThawingPeriod(uint64 period) external;

    /// @notice Register a trusted state root for a given block hash.
    /// @dev Called by governance or an authorised oracle after verifying the block header.
    ///      On Arbitrum, L1 block hashes are not natively available, so we rely on a
    ///      trusted oracle. The stateRoot is then used to validate EIP-1186 MPT proofs.
    /// @param blockHash  The Ethereum block hash.
    /// @param stateRoot  The corresponding EIP-1186 state root.
    function setTrustedStateRoot(bytes32 blockHash, bytes32 stateRoot) external;

    /// @notice Set the GRT issuance rate per compute unit.
    /// @dev Rate of 0 disables issuance.
    function setIssuancePerCU(uint256 rate) external;

    /// @notice Deposit GRT into the rewards pool used for provider issuance payments.
    /// @dev Caller must have approved `amount` GRT to this contract.
    function depositRewardsPool(uint256 amount) external;

    /// @notice Withdraw unused GRT from the rewards pool (owner only).
    function withdrawRewardsPool(uint256 amount) external;

    // -------------------------------------------------------------------------
    // Provider operations
    // -------------------------------------------------------------------------

    /// @notice Claim all accrued GRT rewards for the caller.
    function claimRewards() external;

    /// @notice Update the address that receives collected GRT fees.
    /// @dev Defaults to serviceProvider at registration. Allows separation of
    ///      operator key (used for signing) from payment wallet (cold storage etc.).
    ///      Takes effect on the next collect() call.
    function setPaymentsDestination(address destination) external;

    // -------------------------------------------------------------------------
    // Provider views
    // -------------------------------------------------------------------------

    function isRegistered(address provider) external view returns (bool);

    function getChainRegistrations(address provider) external view returns (ChainRegistration[] memory);

    function activeRegistrationCount(address provider) external view returns (uint256);

    function paymentsDestination(address provider) external view returns (address);

    function pendingRewards(address provider) external view returns (uint256);

    function rewardsPool() external view returns (uint256);

    function minThawingPeriod() external view returns (uint64);

}
