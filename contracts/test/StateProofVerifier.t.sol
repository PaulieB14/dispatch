// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.27;

import {Test} from "forge-std/Test.sol";

import {RLP} from "@openzeppelin/contracts/utils/RLP.sol";

import {StateProofVerifier} from "../src/lib/StateProofVerifier.sol";
import {RPCDataService} from "../src/RPCDataService.sol";
import {IRPCDataService} from "../src/interfaces/IRPCDataService.sol";
import {IHorizonStakingTypes} from "@graphprotocol/interfaces/contracts/horizon/internal/IHorizonStakingTypes.sol";

// ---------------------------------------------------------------------------
// Helpers — re-use the mocks from the main test file inline.
// ---------------------------------------------------------------------------

contract MockHorizonStakingForSlash {
    mapping(address => mapping(address => IHorizonStakingTypes.Provision)) public provisions;

    // Tracks slash calls for assertion.
    address public lastSlashedProvider;
    uint256 public lastSlashTokens;
    uint256 public lastSlashVerifier;
    address public lastSlashDest;

    function setProvision(address sp, address ds, uint256 tokens, uint64 thawing) external {
        provisions[sp][ds] = IHorizonStakingTypes.Provision({
            tokens: tokens,
            tokensThawing: 0,
            sharesThawing: 0,
            maxVerifierCut: 1_000_000,
            thawingPeriod: thawing,
            createdAt: uint64(block.timestamp),
            maxVerifierCutPending: 0,
            thawingPeriodPending: 0,
            lastParametersStagedAt: 0,
            thawingNonce: 0
        });
    }

    function getProvision(address sp, address ds) external view returns (IHorizonStakingTypes.Provision memory) {
        return provisions[sp][ds];
    }

    function isAuthorized(address sp, address, address op) external pure returns (bool) {
        return sp == op;
    }

    function slash(address sp, uint256 tokens, uint256 verifier, address dest) external {
        lastSlashedProvider = sp;
        lastSlashTokens = tokens;
        lastSlashVerifier = verifier;
        lastSlashDest = dest;
    }

    function acceptProvisionParameters(address) external {}
}

contract MockControllerForSlash {
    address public staking_;

    constructor(address _staking) {
        staking_ = _staking;
    }

    mapping(bytes32 => address) private _contracts;

    function init(address dummy) external {
        _contracts[keccak256("GraphToken")] = dummy;
        _contracts[keccak256("Staking")] = staking_;
        _contracts[keccak256("GraphPayments")] = dummy;
        _contracts[keccak256("PaymentsEscrow")] = dummy;
        _contracts[keccak256("EpochManager")] = dummy;
        _contracts[keccak256("RewardsManager")] = dummy;
        _contracts[keccak256("GraphTokenGateway")] = dummy;
        _contracts[keccak256("GraphProxyAdmin")] = dummy;
        _contracts[keccak256("Curation")] = dummy;
    }

    function getContractProxy(bytes32 id) external view returns (address) {
        return _contracts[id];
    }
}

// ---------------------------------------------------------------------------
// External wrapper so forge can catch reverts from an internal library call.
// ---------------------------------------------------------------------------

contract DecodeAccountWrapper {
    function decode(bytes memory rlp) external pure returns (StateProofVerifier.Account memory) {
        return StateProofVerifier.decodeAccount(rlp);
    }
}

// ---------------------------------------------------------------------------
// StateProofVerifier unit tests
// ---------------------------------------------------------------------------

contract StateProofVerifierTest is Test {
    DecodeAccountWrapper wrapper = new DecodeAccountWrapper();

    // -----------------------------------------------------------------------
    // decodeAccount — round-trip via OZ RLP encoder
    // -----------------------------------------------------------------------

    /// @dev Builds a 4-field RLP account list from raw values using the OZ encoder.
    function _buildRlpAccount(uint256 nonce, uint256 balance, bytes32 storageRoot, bytes32 codeHash)
        internal
        pure
        returns (bytes memory)
    {
        RLP.Encoder memory enc = RLP.encoder();
        enc = RLP.push(enc, nonce);
        enc = RLP.push(enc, balance);
        enc = RLP.push(enc, storageRoot);
        enc = RLP.push(enc, codeHash);
        return RLP.encode(enc);
    }

    function test_decodeAccount_roundtrip() public pure {
        uint256 nonce = 42;
        uint256 balance = 1_000e18;
        bytes32 storageRoot = bytes32(uint256(0xdeadbeef));
        bytes32 codeHash = bytes32(uint256(0xcafebabe));

        bytes memory rlp = _buildRlpAccount(nonce, balance, storageRoot, codeHash);
        StateProofVerifier.Account memory acc = StateProofVerifier.decodeAccount(rlp);

        assertEq(acc.nonce, nonce);
        assertEq(acc.balance, balance);
        assertEq(acc.storageRoot, storageRoot);
        assertEq(acc.codeHash, codeHash);
    }

    function test_decodeAccount_zeroNonce() public pure {
        bytes memory rlp = _buildRlpAccount(0, 0, bytes32(0), bytes32(0));
        StateProofVerifier.Account memory acc = StateProofVerifier.decodeAccount(rlp);
        assertEq(acc.nonce, 0);
        assertEq(acc.balance, 0);
    }

    function test_decodeAccount_revertOnInvalidRlp() public {
        bytes memory bad = hex"c3010203"; // 3-field list — wrong
        vm.expectRevert("StateProofVerifier: invalid account RLP");
        wrapper.decode(bad);
    }
}

// ---------------------------------------------------------------------------
// slash() error-path tests (no real trie proof needed)
// ---------------------------------------------------------------------------

contract SlashErrorPathTest is Test {
    RPCDataService public service;
    MockHorizonStakingForSlash public staking;

    address public owner = makeAddr("owner");
    address public guardian = makeAddr("guardian");
    address public provider = makeAddr("provider");
    address public challenger = makeAddr("challenger");

    uint256 constant PROVISION = 25_000e18;
    uint64 constant THAWING = 14 days;
    uint64 constant CHAIN_ID = 1;

    function setUp() public {
        staking = new MockHorizonStakingForSlash();

        MockControllerForSlash ctrl = new MockControllerForSlash(address(staking));
        ctrl.init(address(1)); // dummy for non-staking slots

        service = new RPCDataService(owner, address(ctrl), address(0), guardian, address(0));

        staking.setProvision(provider, address(service), PROVISION, THAWING);

        vm.startPrank(owner);
        service.addChain(CHAIN_ID, 0);
        vm.stopPrank();

        // Register the provider.
        vm.prank(provider);
        service.register(provider, abi.encode("https://rpc.example.com", "u1hx", address(0)));
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    // Fixed address standing in for "some account being disputed".
    address internal constant DISPUTED_ACCOUNT = address(0xacC0);

    function _emptyProof() internal view returns (IRPCDataService.Tier1FraudProof memory p) {
        p.challenger = challenger;
        p.chainId = CHAIN_ID;
        p.account = DISPUTED_ACCOUNT;
        p.blockHash = keccak256("block");
        p.storageSlot = bytes32(0);
        p.accountProof = new bytes[](0);
        p.storageProof = new bytes[](0);
        p.claimedValue = 999;
        p.disputeType = IRPCDataService.DisputeType.Balance;
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    function test_slash_revertIfProviderNotRegistered() public {
        address unknown = makeAddr("unknown");
        IRPCDataService.Tier1FraudProof memory p = _emptyProof();

        vm.expectRevert(abi.encodeWithSelector(IRPCDataService.ProviderNotRegistered.selector, unknown));
        service.slash(unknown, abi.encode(p));
    }

    function test_slash_revertIfBlockHashNotTrusted() public {
        IRPCDataService.Tier1FraudProof memory p = _emptyProof();
        // trustedStateRoots[p.blockHash] is zero — not set.

        vm.expectRevert(abi.encodeWithSelector(IRPCDataService.UntrustedBlockHash.selector, p.blockHash));
        service.slash(provider, abi.encode(p));
    }

    function test_slash_revertWhenPaused() public {
        vm.prank(guardian);
        service.pause();

        IRPCDataService.Tier1FraudProof memory p = _emptyProof();
        vm.expectRevert(); // Pausable: paused
        service.slash(provider, abi.encode(p));
    }

    function test_setTrustedStateRoot_onlyOwner() public {
        bytes32 bh = keccak256("block");
        bytes32 sr = keccak256("state");

        vm.prank(makeAddr("attacker"));
        vm.expectRevert(); // Ownable
        service.setTrustedStateRoot(bh, sr);
    }

    function test_setTrustedStateRoot_storesAndEmits() public {
        bytes32 bh = keccak256("block");
        bytes32 sr = keccak256("state");

        vm.expectEmit(true, false, false, true);
        emit RPCDataService.TrustedStateRootSet(bh, sr);

        vm.prank(owner);
        service.setTrustedStateRoot(bh, sr);

        assertEq(service.trustedStateRoots(bh), sr);
    }

    function test_slashConstants() public view {
        assertEq(service.SLASH_AMOUNT(), 10_000e18);
        assertEq(service.CHALLENGER_REWARD_PPM(), 500_000);
    }
}
