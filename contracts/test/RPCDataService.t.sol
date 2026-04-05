// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.27;

import {Test, console2} from "forge-std/Test.sol";

import {RPCDataService} from "../src/RPCDataService.sol";
import {IRPCDataService} from "../src/interfaces/IRPCDataService.sol";
import {IHorizonStaking} from "@graphprotocol/horizon/interfaces/IHorizonStaking.sol";
import {IHorizonStakingTypes} from "@graphprotocol/interfaces/contracts/horizon/internal/IHorizonStakingTypes.sol";
import {IHorizonStakingMain} from "@graphprotocol/interfaces/contracts/horizon/internal/IHorizonStakingMain.sol";
import {IHorizonStakingBase} from "@graphprotocol/interfaces/contracts/horizon/internal/IHorizonStakingBase.sol";
import {IGraphPayments} from "@graphprotocol/horizon/interfaces/IGraphPayments.sol";

/// @dev Minimal mock of IHorizonStaking — only the provision-related methods used by RPCDataService.
contract MockHorizonStaking {
    mapping(address => mapping(address => IHorizonStakingTypes.Provision)) public provisions;

    function setProvision(
        address serviceProvider,
        address dataService,
        uint256 tokens,
        uint64 thawingPeriod_
    ) external {
        provisions[serviceProvider][dataService] = IHorizonStakingTypes.Provision({
            tokens: tokens,
            tokensThawing: 0,
            sharesThawing: 0,
            maxVerifierCut: 1_000_000,
            thawingPeriod: thawingPeriod_,
            createdAt: uint64(block.timestamp),
            maxVerifierCutPending: 0,
            thawingPeriodPending: 0,
            lastParametersStagedAt: 0,
            thawingNonce: 0
        });
    }

    function getProvision(address serviceProvider, address dataService)
        external
        view
        returns (IHorizonStakingTypes.Provision memory)
    {
        return provisions[serviceProvider][dataService];
    }

    function isAuthorized(address serviceProvider, address, address operator)
        external
        pure
        returns (bool)
    {
        return serviceProvider == operator;
    }

    function slash(address, uint256, uint256, address) external {}
    function acceptProvisionParameters(address) external {}
}

/// @dev Mock IController — returns the staking address for "Staking", and address(1) for everything else.
/// GraphDirectory calls getContractProxy(keccak256(name)) in its constructor.
contract MockController {
    mapping(bytes32 => address) private _contracts;

    constructor(address staking_) {
        address dummy = address(1);
        _contracts[keccak256("GraphToken")]         = dummy;
        _contracts[keccak256("Staking")]            = staking_;
        _contracts[keccak256("GraphPayments")]      = dummy;
        _contracts[keccak256("PaymentsEscrow")]     = dummy;
        _contracts[keccak256("EpochManager")]       = dummy;
        _contracts[keccak256("RewardsManager")]     = dummy;
        _contracts[keccak256("GraphTokenGateway")]  = dummy;
        _contracts[keccak256("GraphProxyAdmin")]    = dummy;
        _contracts[keccak256("Curation")]           = dummy;
    }

    function getContractProxy(bytes32 id) external view returns (address) {
        return _contracts[id];
    }
}

contract RPCDataServiceTest is Test {
    RPCDataService public service;
    MockHorizonStaking public staking;
    MockController public controller;

    address public owner = makeAddr("owner");
    address public pauseGuardian = makeAddr("pauseGuardian");
    address public provider = makeAddr("provider");
    address public gateway = makeAddr("gateway");

    uint256 constant SUFFICIENT_PROVISION = 25_000e18;
    uint64 constant SUFFICIENT_THAWING = 14 days;
    uint64 constant CHAIN_ETH_MAINNET = 1;
    uint64 constant CHAIN_ARBITRUM = 42161;

    function setUp() public {
        staking = new MockHorizonStaking();
        controller = new MockController(address(staking));

        // Deploy RPCDataService. address(0) for graphTallyCollector — collect() not tested here.
        service = new RPCDataService(owner, address(controller), address(0), pauseGuardian);

        // Pre-populate staking mock with valid provision for `provider`.
        staking.setProvision(provider, address(service), SUFFICIENT_PROVISION, SUFFICIENT_THAWING);

        // Add supported chains (owner-only).
        vm.startPrank(owner);
        service.addChain(CHAIN_ETH_MAINNET, 0);
        service.addChain(CHAIN_ARBITRUM, 0);
        vm.stopPrank();
    }

    // -------------------------------------------------------------------------
    // Chain governance
    // -------------------------------------------------------------------------

    function test_addChain_setsDefaultMinProvision() public view {
        (bool enabled, uint256 minTokens) = _getChainConfig(CHAIN_ETH_MAINNET);
        assertTrue(enabled);
        assertEq(minTokens, RPCDataService(address(service)).DEFAULT_MIN_PROVISION());
    }

    function test_addChain_customMinProvision() public {
        uint256 customMin = 10_000e18;
        vm.prank(owner);
        service.addChain(999, customMin);

        (bool enabled, uint256 minTokens) = _getChainConfig(999);
        assertTrue(enabled);
        assertEq(minTokens, customMin);
    }

    function test_removeChain_disablesChain() public {
        vm.prank(owner);
        service.removeChain(CHAIN_ETH_MAINNET);

        (bool enabled,) = _getChainConfig(CHAIN_ETH_MAINNET);
        assertFalse(enabled);
    }

    function test_addChain_revertIfNotOwner() public {
        vm.prank(makeAddr("attacker"));
        vm.expectRevert(); // Ownable: caller is not the owner
        service.addChain(1, 0);
    }

    // -------------------------------------------------------------------------
    // Provider registration
    // -------------------------------------------------------------------------

    function test_register_succeeds() public {
        _register(provider, "https://rpc.example.com", "u1hx");
        assertTrue(service.isRegistered(provider));
    }

    function test_register_emitsEvent() public {
        vm.expectEmit(true, false, false, true);
        emit IRPCDataService.ProviderRegistered(provider, "https://rpc.example.com", "u1hx");
        _register(provider, "https://rpc.example.com", "u1hx");
    }

    function test_register_revertIfAlreadyRegistered() public {
        _register(provider, "https://rpc.example.com", "u1hx");
        vm.expectRevert(
            abi.encodeWithSelector(IRPCDataService.ProviderAlreadyRegistered.selector, provider)
        );
        _register(provider, "https://rpc.example.com", "u1hx");
    }

    function test_register_revertIfInsufficientProvision() public {
        address poorProvider = makeAddr("poorProvider");
        staking.setProvision(poorProvider, address(service), SUFFICIENT_PROVISION - 1, SUFFICIENT_THAWING);

        vm.prank(poorProvider);
        vm.expectRevert(); // ProvisionManagerInvalidValue("tokens", ...)
        service.register(poorProvider, abi.encode("https://rpc.example.com", "u1hx"));
    }

    function test_register_revertIfThawingPeriodTooShort() public {
        address shortProvider = makeAddr("shortProvider");
        staking.setProvision(shortProvider, address(service), SUFFICIENT_PROVISION, SUFFICIENT_THAWING - 1);

        vm.prank(shortProvider);
        vm.expectRevert(); // ProvisionManagerInvalidValue("thawingPeriod", ...)
        service.register(shortProvider, abi.encode("https://rpc.example.com", "u1hx"));
    }

    // -------------------------------------------------------------------------
    // Service start / stop
    // -------------------------------------------------------------------------

    function test_startService_succeeds() public {
        _register(provider, "https://rpc.example.com", "u1hx");
        _startService(provider, CHAIN_ETH_MAINNET, IRPCDataService.CapabilityTier.Standard, "https://rpc.example.com");

        IRPCDataService.ChainRegistration[] memory regs = service.getChainRegistrations(provider);
        assertEq(regs.length, 1);
        assertEq(regs[0].chainId, CHAIN_ETH_MAINNET);
        assertTrue(regs[0].active);
    }

    function test_startService_revertIfChainNotSupported() public {
        _register(provider, "https://rpc.example.com", "u1hx");

        vm.prank(provider);
        vm.expectRevert(
            abi.encodeWithSelector(IRPCDataService.ChainNotSupported.selector, uint256(999))
        );
        service.startService(
            provider,
            abi.encode(uint64(999), uint8(IRPCDataService.CapabilityTier.Standard), "https://rpc.example.com")
        );
    }

    function test_startService_revertIfNotRegistered() public {
        vm.prank(provider);
        vm.expectRevert(
            abi.encodeWithSelector(IRPCDataService.ProviderNotRegistered.selector, provider)
        );
        service.startService(
            provider,
            abi.encode(CHAIN_ETH_MAINNET, uint8(IRPCDataService.CapabilityTier.Standard), "https://rpc.example.com")
        );
    }

    function test_stopService_deactivatesRegistration() public {
        _register(provider, "https://rpc.example.com", "u1hx");
        _startService(provider, CHAIN_ETH_MAINNET, IRPCDataService.CapabilityTier.Standard, "https://rpc.example.com");

        vm.prank(provider);
        service.stopService(
            provider,
            abi.encode(CHAIN_ETH_MAINNET, uint8(IRPCDataService.CapabilityTier.Standard))
        );

        IRPCDataService.ChainRegistration[] memory regs = service.getChainRegistrations(provider);
        assertFalse(regs[0].active);
        assertEq(service.activeRegistrationCount(provider), 0);
    }

    function test_stopService_revertIfNotFound() public {
        _register(provider, "https://rpc.example.com", "u1hx");

        vm.prank(provider);
        vm.expectRevert(
            abi.encodeWithSelector(
                IRPCDataService.RegistrationNotFound.selector,
                provider,
                CHAIN_ETH_MAINNET,
                IRPCDataService.CapabilityTier.Standard
            )
        );
        service.stopService(
            provider,
            abi.encode(CHAIN_ETH_MAINNET, uint8(IRPCDataService.CapabilityTier.Standard))
        );
    }

    function test_deregister_revertIfActiveRegistrationsExist() public {
        _register(provider, "https://rpc.example.com", "u1hx");
        _startService(provider, CHAIN_ETH_MAINNET, IRPCDataService.CapabilityTier.Standard, "https://rpc.example.com");

        vm.prank(provider);
        vm.expectRevert(
            abi.encodeWithSelector(IRPCDataService.ActiveRegistrationsExist.selector, provider)
        );
        service.deregister(provider, "");
    }

    // -------------------------------------------------------------------------
    // Pause guardian
    // -------------------------------------------------------------------------

    function test_pause_blocksRegister() public {
        vm.prank(pauseGuardian);
        service.pause();

        vm.prank(provider);
        vm.expectRevert(); // Pausable: paused
        service.register(provider, abi.encode("https://rpc.example.com", "u1hx"));
    }

    function test_unpause_allowsRegister() public {
        vm.prank(pauseGuardian);
        service.pause();
        vm.prank(pauseGuardian);
        service.unpause();

        _register(provider, "https://rpc.example.com", "u1hx");
        assertTrue(service.isRegistered(provider));
    }

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    function _register(address _provider, string memory endpoint, string memory geo) internal {
        vm.prank(_provider);
        service.register(_provider, abi.encode(endpoint, geo));
    }

    function _startService(
        address _provider,
        uint64 chainId,
        IRPCDataService.CapabilityTier tier,
        string memory endpoint
    ) internal {
        vm.prank(_provider);
        service.startService(_provider, abi.encode(chainId, uint8(tier), endpoint));
    }

    function _setProvision(address _provider, uint256 tokens, uint64 thawingPeriod) internal {
        staking.setProvision(_provider, address(service), tokens, thawingPeriod);
    }

    function _getChainConfig(uint256 chainId)
        internal
        view
        returns (bool enabled, uint256 minTokens)
    {
        (enabled, minTokens) = service.supportedChains(chainId);
    }
}
