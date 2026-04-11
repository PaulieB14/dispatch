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

/// @dev Minimal ERC-20 mock for GRT bond tests.
contract MockERC20 {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        return true;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        require(balanceOf[msg.sender] >= amount, "insufficient");
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        require(balanceOf[from] >= amount, "insufficient");
        require(allowance[from][msg.sender] >= amount, "insufficient allowance");
        balanceOf[from] -= amount;
        allowance[from][msg.sender] -= amount;
        balanceOf[to] += amount;
        return true;
    }
}

/// @dev Minimal mock of IHorizonStaking — only the provision-related methods used by RPCDataService.
contract MockHorizonStaking {
    mapping(address => mapping(address => IHorizonStakingTypes.Provision)) public provisions;

    function setProvision(address serviceProvider, address dataService, uint256 tokens, uint64 thawingPeriod_)
        external
    {
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

    function isAuthorized(address serviceProvider, address, address operator) external pure returns (bool) {
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

contract RPCDataServiceTest is Test {
    RPCDataService public service;
    MockHorizonStaking public staking;
    MockController public controller;
    MockERC20 public grt;

    address public owner = makeAddr("owner");
    address public pauseGuardian = makeAddr("pauseGuardian");
    address public provider = makeAddr("provider");
    address public gateway = makeAddr("gateway");
    address public proposer = makeAddr("proposer");

    uint256 constant SUFFICIENT_PROVISION = 25_000e18;
    uint64 constant SUFFICIENT_THAWING = 14 days;
    uint64 constant CHAIN_ETH_MAINNET = 1;
    uint64 constant CHAIN_ARBITRUM = 42161;

    function setUp() public {
        staking = new MockHorizonStaking();
        controller = new MockController(address(staking));
        grt = new MockERC20();

        // Deploy RPCDataService. address(0) for graphTallyCollector — collect() not tested here.
        service = new RPCDataService(owner, address(controller), address(0), pauseGuardian, address(grt));

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
        _register(provider, "https://rpc.example.com", "u1hx", address(0));
        assertTrue(service.isRegistered(provider));
    }

    function test_register_defaultsPaymentsDestinationToProvider() public {
        _register(provider, "https://rpc.example.com", "u1hx", address(0));
        assertEq(service.paymentsDestination(provider), provider);
    }

    function test_register_setsCustomPaymentsDestination() public {
        address wallet = makeAddr("paymentWallet");
        _register(provider, "https://rpc.example.com", "u1hx", wallet);
        assertEq(service.paymentsDestination(provider), wallet);
    }

    function test_register_emitsEvent() public {
        vm.expectEmit(true, false, false, true);
        emit IRPCDataService.ProviderRegistered(provider, "https://rpc.example.com", "u1hx");
        _register(provider, "https://rpc.example.com", "u1hx", address(0));
    }

    function test_register_revertIfAlreadyRegistered() public {
        _register(provider, "https://rpc.example.com", "u1hx", address(0));
        vm.expectRevert(abi.encodeWithSelector(IRPCDataService.ProviderAlreadyRegistered.selector, provider));
        _register(provider, "https://rpc.example.com", "u1hx", address(0));
    }

    function test_register_revertIfInsufficientProvision() public {
        address poorProvider = makeAddr("poorProvider");
        staking.setProvision(poorProvider, address(service), SUFFICIENT_PROVISION - 1, SUFFICIENT_THAWING);

        vm.prank(poorProvider);
        vm.expectRevert(); // ProvisionManagerInvalidValue("tokens", ...)
        service.register(poorProvider, abi.encode("https://rpc.example.com", "u1hx", address(0)));
    }

    function test_register_revertIfThawingPeriodTooShort() public {
        address shortProvider = makeAddr("shortProvider");
        staking.setProvision(shortProvider, address(service), SUFFICIENT_PROVISION, SUFFICIENT_THAWING - 1);

        vm.prank(shortProvider);
        vm.expectRevert(); // ProvisionManagerInvalidValue("thawingPeriod", ...)
        service.register(shortProvider, abi.encode("https://rpc.example.com", "u1hx", address(0)));
    }

    // -------------------------------------------------------------------------
    // setPaymentsDestination
    // -------------------------------------------------------------------------

    function test_setPaymentsDestination_updatesDestination() public {
        _register(provider, "https://rpc.example.com", "u1hx", address(0));
        address newWallet = makeAddr("newWallet");

        vm.prank(provider);
        service.setPaymentsDestination(newWallet);

        assertEq(service.paymentsDestination(provider), newWallet);
    }

    function test_setPaymentsDestination_zeroAddressResetsToSelf() public {
        address wallet = makeAddr("wallet");
        _register(provider, "https://rpc.example.com", "u1hx", wallet);

        vm.prank(provider);
        service.setPaymentsDestination(address(0));

        assertEq(service.paymentsDestination(provider), provider);
    }

    function test_setPaymentsDestination_emitsEvent() public {
        _register(provider, "https://rpc.example.com", "u1hx", address(0));
        address newWallet = makeAddr("newWallet");

        vm.expectEmit(true, true, false, false);
        emit IRPCDataService.PaymentsDestinationSet(provider, newWallet);

        vm.prank(provider);
        service.setPaymentsDestination(newWallet);
    }

    function test_setPaymentsDestination_revertIfNotRegistered() public {
        vm.prank(provider);
        vm.expectRevert(abi.encodeWithSelector(IRPCDataService.ProviderNotRegistered.selector, provider));
        service.setPaymentsDestination(makeAddr("wallet"));
    }

    // -------------------------------------------------------------------------
    // Service start / stop
    // -------------------------------------------------------------------------

    function test_startService_succeeds() public {
        _register(provider, "https://rpc.example.com", "u1hx", address(0));
        _startService(provider, CHAIN_ETH_MAINNET, IRPCDataService.CapabilityTier.Standard, "https://rpc.example.com");

        IRPCDataService.ChainRegistration[] memory regs = service.getChainRegistrations(provider);
        assertEq(regs.length, 1);
        assertEq(regs[0].chainId, CHAIN_ETH_MAINNET);
        assertTrue(regs[0].active);
    }

    function test_startService_revertIfChainNotSupported() public {
        _register(provider, "https://rpc.example.com", "u1hx", address(0));

        vm.prank(provider);
        vm.expectRevert(abi.encodeWithSelector(IRPCDataService.ChainNotSupported.selector, uint256(999)));
        service.startService(
            provider, abi.encode(uint64(999), uint8(IRPCDataService.CapabilityTier.Standard), "https://rpc.example.com")
        );
    }

    function test_startService_revertIfNotRegistered() public {
        vm.prank(provider);
        vm.expectRevert(abi.encodeWithSelector(IRPCDataService.ProviderNotRegistered.selector, provider));
        service.startService(
            provider,
            abi.encode(CHAIN_ETH_MAINNET, uint8(IRPCDataService.CapabilityTier.Standard), "https://rpc.example.com")
        );
    }

    function test_stopService_deactivatesRegistration() public {
        _register(provider, "https://rpc.example.com", "u1hx", address(0));
        _startService(provider, CHAIN_ETH_MAINNET, IRPCDataService.CapabilityTier.Standard, "https://rpc.example.com");

        vm.prank(provider);
        service.stopService(provider, abi.encode(CHAIN_ETH_MAINNET, uint8(IRPCDataService.CapabilityTier.Standard)));

        IRPCDataService.ChainRegistration[] memory regs = service.getChainRegistrations(provider);
        assertFalse(regs[0].active);
        assertEq(service.activeRegistrationCount(provider), 0);
    }

    function test_stopService_revertIfNotFound() public {
        _register(provider, "https://rpc.example.com", "u1hx", address(0));

        vm.prank(provider);
        vm.expectRevert(
            abi.encodeWithSelector(
                IRPCDataService.RegistrationNotFound.selector,
                provider,
                CHAIN_ETH_MAINNET,
                IRPCDataService.CapabilityTier.Standard
            )
        );
        service.stopService(provider, abi.encode(CHAIN_ETH_MAINNET, uint8(IRPCDataService.CapabilityTier.Standard)));
    }

    function test_deregister_revertIfActiveRegistrationsExist() public {
        _register(provider, "https://rpc.example.com", "u1hx", address(0));
        _startService(provider, CHAIN_ETH_MAINNET, IRPCDataService.CapabilityTier.Standard, "https://rpc.example.com");

        vm.prank(provider);
        vm.expectRevert(abi.encodeWithSelector(IRPCDataService.ActiveRegistrationsExist.selector, provider));
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
        service.register(provider, abi.encode("https://rpc.example.com", "u1hx", address(0)));
    }

    function test_unpause_allowsRegister() public {
        vm.prank(pauseGuardian);
        service.pause();
        vm.prank(pauseGuardian);
        service.unpause();

        _register(provider, "https://rpc.example.com", "u1hx", address(0));
        assertTrue(service.isRegistered(provider));
    }

    // -------------------------------------------------------------------------
    // Permissionless chain proposals
    // -------------------------------------------------------------------------

    function test_proposeChain_locksGrtBond() public {
        uint256 bond = RPCDataService(address(service)).CHAIN_BOND_AMOUNT();
        grt.mint(proposer, bond);
        vm.prank(proposer);
        grt.approve(address(service), bond);
        vm.prank(proposer);
        service.proposeChain(999);

        (address storedProposer, uint256 storedAmount,) = RPCDataService(address(service)).pendingChainBonds(999);
        assertEq(storedProposer, proposer);
        assertEq(storedAmount, bond);
        assertEq(grt.balanceOf(address(service)), bond);
    }

    function test_proposeChain_emitsEvent() public {
        uint256 bond = RPCDataService(address(service)).CHAIN_BOND_AMOUNT();
        grt.mint(proposer, bond);
        vm.prank(proposer);
        grt.approve(address(service), bond);

        vm.expectEmit(true, true, false, true);
        emit IRPCDataService.ChainProposed(999, proposer, bond);
        vm.prank(proposer);
        service.proposeChain(999);
    }

    function test_proposeChain_revertIfAlreadySupported() public {
        vm.expectRevert(abi.encodeWithSelector(IRPCDataService.ChainAlreadySupported.selector, CHAIN_ETH_MAINNET));
        vm.prank(proposer);
        service.proposeChain(CHAIN_ETH_MAINNET);
    }

    function test_proposeChain_revertIfAlreadyPending() public {
        uint256 bond = RPCDataService(address(service)).CHAIN_BOND_AMOUNT();
        grt.mint(proposer, bond * 2);
        vm.startPrank(proposer);
        grt.approve(address(service), bond * 2);
        service.proposeChain(999);
        vm.expectRevert(abi.encodeWithSelector(IRPCDataService.ChainAlreadyProposed.selector, uint256(999)));
        service.proposeChain(999);
        vm.stopPrank();
    }

    function test_approveProposedChain_enablesChainAndRefundsBond() public {
        uint256 bond = RPCDataService(address(service)).CHAIN_BOND_AMOUNT();
        grt.mint(proposer, bond);
        vm.prank(proposer);
        grt.approve(address(service), bond);
        vm.prank(proposer);
        service.proposeChain(999);

        vm.prank(owner);
        service.approveProposedChain(999, 0);

        (bool enabled,) = _getChainConfig(999);
        assertTrue(enabled);
        assertEq(grt.balanceOf(proposer), bond); // bond refunded
        assertEq(grt.balanceOf(address(service)), 0);
    }

    function test_rejectProposedChain_forfeitsToTreasury() public {
        uint256 bond = RPCDataService(address(service)).CHAIN_BOND_AMOUNT();
        grt.mint(proposer, bond);
        vm.prank(proposer);
        grt.approve(address(service), bond);
        vm.prank(proposer);
        service.proposeChain(999);

        uint256 ownerBefore = grt.balanceOf(owner);
        vm.prank(owner);
        service.rejectProposedChain(999);

        assertEq(grt.balanceOf(owner), ownerBefore + bond);
        assertEq(grt.balanceOf(proposer), 0);

        (address storedProposer,,) = RPCDataService(address(service)).pendingChainBonds(999);
        assertEq(storedProposer, address(0)); // cleared
    }

    function test_approveProposedChain_revertIfNotPending() public {
        vm.prank(owner);
        vm.expectRevert(abi.encodeWithSelector(IRPCDataService.ChainNotProposed.selector, uint256(999)));
        service.approveProposedChain(999, 0);
    }

    // -------------------------------------------------------------------------
    // GRT issuance rate
    // -------------------------------------------------------------------------

    function test_setIssuancePerCU_storesRate() public {
        vm.prank(owner);
        service.setIssuancePerCU(1e12);
        assertEq(RPCDataService(address(service)).issuancePerCU(), 1e12);
    }

    function test_setIssuancePerCU_revertIfNotOwner() public {
        vm.prank(makeAddr("attacker"));
        vm.expectRevert();
        service.setIssuancePerCU(1e12);
    }

    // -------------------------------------------------------------------------
    // Dynamic thawing period
    // -------------------------------------------------------------------------

    function test_setMinThawingPeriod_storesPeriod() public {
        uint64 newPeriod = 28 days;
        vm.prank(owner);
        service.setMinThawingPeriod(newPeriod);
        assertEq(RPCDataService(address(service)).minThawingPeriod(), newPeriod);
    }

    function test_setMinThawingPeriod_emitsEvent() public {
        uint64 newPeriod = 21 days;
        vm.expectEmit(false, false, false, true);
        emit IRPCDataService.MinThawingPeriodSet(newPeriod);
        vm.prank(owner);
        service.setMinThawingPeriod(newPeriod);
    }

    function test_setMinThawingPeriod_revertIfTooShort() public {
        uint64 tooShort = 14 days - 1;
        vm.prank(owner);
        vm.expectRevert(
            abi.encodeWithSelector(IRPCDataService.ThawingPeriodTooShort.selector, uint64(14 days), tooShort)
        );
        service.setMinThawingPeriod(tooShort);
    }

    function test_setMinThawingPeriod_revertIfNotOwner() public {
        vm.prank(makeAddr("attacker"));
        vm.expectRevert();
        service.setMinThawingPeriod(28 days);
    }

    function test_minThawingPeriod_initializedToConstant() public view {
        assertEq(RPCDataService(address(service)).minThawingPeriod(), RPCDataService(address(service)).MIN_THAWING_PERIOD());
    }

    // -------------------------------------------------------------------------
    // Rewards pool — deposit / withdraw
    // -------------------------------------------------------------------------

    function test_depositRewardsPool_transfersGrtAndUpdatesPool() public {
        uint256 amount = 500_000e18;
        grt.mint(owner, amount);
        vm.startPrank(owner);
        grt.approve(address(service), amount);
        service.depositRewardsPool(amount);
        vm.stopPrank();

        assertEq(RPCDataService(address(service)).rewardsPool(), amount);
        assertEq(grt.balanceOf(address(service)), amount);
    }

    function test_depositRewardsPool_emitsEvent() public {
        uint256 amount = 100_000e18;
        grt.mint(owner, amount);
        vm.startPrank(owner);
        grt.approve(address(service), amount);
        vm.expectEmit(false, false, false, true);
        emit IRPCDataService.RewardsDeposited(amount);
        service.depositRewardsPool(amount);
        vm.stopPrank();
    }

    function test_depositRewardsPool_revertIfNotOwner() public {
        vm.prank(makeAddr("attacker"));
        vm.expectRevert();
        service.depositRewardsPool(1e18);
    }

    function test_withdrawRewardsPool_transfersGrtAndReducesPool() public {
        uint256 deposit = 500_000e18;
        uint256 withdraw = 200_000e18;
        grt.mint(owner, deposit);
        vm.startPrank(owner);
        grt.approve(address(service), deposit);
        service.depositRewardsPool(deposit);
        service.withdrawRewardsPool(withdraw);
        vm.stopPrank();

        assertEq(RPCDataService(address(service)).rewardsPool(), deposit - withdraw);
        assertEq(grt.balanceOf(owner), withdraw);
    }

    function test_withdrawRewardsPool_emitsEvent() public {
        uint256 amount = 100_000e18;
        grt.mint(owner, amount);
        vm.startPrank(owner);
        grt.approve(address(service), amount);
        service.depositRewardsPool(amount);
        vm.expectEmit(false, false, false, true);
        emit IRPCDataService.RewardsWithdrawn(amount);
        service.withdrawRewardsPool(amount);
        vm.stopPrank();
    }

    function test_withdrawRewardsPool_revertIfInsufficient() public {
        vm.prank(owner);
        vm.expectRevert(
            abi.encodeWithSelector(IRPCDataService.InsufficientRewardsPool.selector, uint256(0), uint256(1e18))
        );
        service.withdrawRewardsPool(1e18);
    }

    function test_withdrawRewardsPool_revertIfNotOwner() public {
        vm.prank(makeAddr("attacker"));
        vm.expectRevert();
        service.withdrawRewardsPool(1e18);
    }

    // -------------------------------------------------------------------------
    // Rewards pool — claimRewards
    // -------------------------------------------------------------------------

    function test_claimRewards_revertIfNoPendingRewards() public {
        vm.prank(provider);
        vm.expectRevert(abi.encodeWithSelector(IRPCDataService.NoPendingRewards.selector, provider));
        service.claimRewards();
    }

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    function _register(address _provider, string memory endpoint, string memory geo, address dest) internal {
        vm.prank(_provider);
        service.register(_provider, abi.encode(endpoint, geo, dest));
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

    function _getChainConfig(uint256 chainId) internal view returns (bool enabled, uint256 minTokens) {
        (enabled, minTokens) = service.supportedChains(chainId);
    }
}
