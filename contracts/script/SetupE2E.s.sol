// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.27;

import {Script} from "forge-std/Script.sol";
import {Vm} from "forge-std/Vm.sol";
import {MessageHashUtils} from "@openzeppelin/contracts/utils/cryptography/MessageHashUtils.sol";
import {TransparentUpgradeableProxy} from "@openzeppelin/contracts/proxy/transparent/TransparentUpgradeableProxy.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";

import {RPCDataService} from "../src/RPCDataService.sol";
import {IHorizonStakingTypes} from "@graphprotocol/interfaces/contracts/horizon/internal/IHorizonStakingTypes.sol";

import {GraphPayments} from "@graphprotocol/horizon/payments/GraphPayments.sol";
import {PaymentsEscrow} from "@graphprotocol/horizon/payments/PaymentsEscrow.sol";
import {GraphTallyCollector} from "@graphprotocol/horizon/payments/collectors/GraphTallyCollector.sol";
import {MockGRTToken} from "@graphprotocol/horizon/mocks/MockGRTToken.sol";
import {ControllerMock} from "@graphprotocol/horizon/mocks/ControllerMock.sol";

// ---------------------------------------------------------------------------
// MockHorizonStakingIntegration — copied verbatim from
// test/RPCDataService.integration.t.sol so the broadcast script can deploy it
// without depending on the test target.
// ---------------------------------------------------------------------------
contract MockHorizonStakingIntegration {
    mapping(address => mapping(address => IHorizonStakingTypes.Provision)) public provisions;

    function setProvision(address serviceProvider, address dataService, uint256 tokens, uint64 thawingPeriod) external {
        provisions[serviceProvider][dataService] = IHorizonStakingTypes.Provision({
            tokens: tokens,
            tokensThawing: 0,
            sharesThawing: 0,
            maxVerifierCut: 1_000_000,
            thawingPeriod: thawingPeriod,
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

    function getProviderTokensAvailable(address serviceProvider, address dataService) external view returns (uint256) {
        return provisions[serviceProvider][dataService].tokens;
    }

    function getTokensAvailable(address serviceProvider, address dataService, uint32) external view returns (uint256) {
        return provisions[serviceProvider][dataService].tokens;
    }

    function getDelegationPool(address, address) external pure returns (IHorizonStakingTypes.DelegationPool memory) {
        return IHorizonStakingTypes.DelegationPool({
            tokens: 0, shares: 0, tokensThawing: 0, sharesThawing: 0, thawingNonce: 0
        });
    }

    function slash(address, uint256, uint256, address) external {}
    function acceptProvisionParameters(address) external {}
}

// ---------------------------------------------------------------------------
// SetupE2E — full Horizon stack deployment for the TypeScript E2E suite.
//
// Run from the contracts/ directory:
//   forge script script/SetupE2E.s.sol --rpc-url http://127.0.0.1:8545 --broadcast --skip-simulation
//
// Writes the deployed addresses + actor keys to out/e2e-fixture.json so the
// vitest setup can spin up the Rust binaries against the right contracts.
// ---------------------------------------------------------------------------
contract SetupE2E is Script {
    // -------------------------------------------------------------------
    // Deterministic Anvil keys (default --accounts mnemonic, indices 0-3).
    //   0: deployer / owner          0xf39Fd6e51aad88F6F4ce6aB8827279cfFFb92266
    //   1: provider                  0x70997970C51812dc3A010C7d01b50e0d17dc79C8
    //   2: gateway (TAP payer)       0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC
    //   3: gateway signer            0x90F79bf6EB2c4f870365E785982E1f101E93b906
    //   4: paymentWallet (no key needed; just an address sink)
    // -------------------------------------------------------------------
    uint256 internal constant DEPLOYER_KEY      = 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80;
    uint256 internal constant PROVIDER_KEY      = 0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d;
    uint256 internal constant GATEWAY_KEY       = 0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a;
    uint256 internal constant GATEWAY_SIGNER_KEY = 0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6;

    uint256 internal constant SUFFICIENT_PROVISION = 10_000e18;
    uint64  internal constant SUFFICIENT_THAWING   = 14 days;
    uint256 internal constant DEPOSIT              = 100_000e18;
    uint64  internal constant CHAIN_ID             = 31337;

    function run() external {
        address deployer       = vm.addr(DEPLOYER_KEY);
        address provider       = vm.addr(PROVIDER_KEY);
        address gateway        = vm.addr(GATEWAY_KEY);
        address gatewaySigner  = vm.addr(GATEWAY_SIGNER_KEY);
        address paymentWallet  = address(uint160(uint256(keccak256("e2e-payment-wallet"))));
        address pauseGuardian  = address(uint160(uint256(keccak256("e2e-pause-guardian"))));

        // ===============================================================
        // Phase 1: deploy as the deployer
        // ===============================================================
        vm.startBroadcast(DEPLOYER_KEY);

        // 1. Base contracts (nonces 0,1,2 on a fresh anvil)
        MockGRTToken grt = new MockGRTToken();
        ControllerMock controller = new ControllerMock(deployer);
        MockHorizonStakingIntegration staking = new MockHorizonStakingIntegration();

        // 2. setContractProxy for the seven non-payment slots (nonces 3..9)
        controller.setContractProxy(keccak256("GraphToken"),         address(grt));
        controller.setContractProxy(keccak256("Staking"),            address(staking));
        controller.setContractProxy(keccak256("EpochManager"),       address(1));
        controller.setContractProxy(keccak256("RewardsManager"),     address(1));
        controller.setContractProxy(keccak256("GraphTokenGateway"),  address(1));
        controller.setContractProxy(keccak256("GraphProxyAdmin"),    address(1));
        controller.setContractProxy(keccak256("Curation"),           address(1));

        // 3. Predict GraphPayments and PaymentsEscrow proxy addresses.
        //    From here:
        //      n+0: setContractProxy("GraphPayments", predicted)
        //      n+1: setContractProxy("PaymentsEscrow", predicted)
        //      n+2: GraphPayments impl   (CREATE)
        //      n+3: GraphPayments proxy  (CREATE)  ← predictedPaymentsProxy
        //      n+4: PaymentsEscrow impl  (CREATE)
        //      n+5: PaymentsEscrow proxy (CREATE)  ← predictedEscrowProxy
        uint64 n = vm.getNonce(deployer);
        address predictedPaymentsProxy = vm.computeCreateAddress(deployer, n + 3);
        address predictedEscrowProxy   = vm.computeCreateAddress(deployer, n + 5);

        controller.setContractProxy(keccak256("GraphPayments"),  predictedPaymentsProxy);
        controller.setContractProxy(keccak256("PaymentsEscrow"), predictedEscrowProxy);

        // 4. GraphPayments (impl + proxy)
        GraphPayments paymentsImpl = new GraphPayments(address(controller), 0);
        GraphPayments payments = GraphPayments(
            address(
                new TransparentUpgradeableProxy(
                    address(paymentsImpl),
                    address(1),
                    abi.encodeCall(GraphPayments.initialize, ())
                )
            )
        );
        require(address(payments) == predictedPaymentsProxy, "payments proxy mismatch");

        // 5. PaymentsEscrow (impl + proxy)
        PaymentsEscrow escrowImpl = new PaymentsEscrow(address(controller), 0);
        PaymentsEscrow escrow = PaymentsEscrow(
            address(
                new TransparentUpgradeableProxy(
                    address(escrowImpl),
                    address(1),
                    abi.encodeCall(PaymentsEscrow.initialize, ())
                )
            )
        );
        require(address(escrow) == predictedEscrowProxy, "escrow proxy mismatch");

        // 6. GraphTallyCollector — non-upgradeable, zero thawing for tests.
        GraphTallyCollector tallyCollector =
            new GraphTallyCollector("GraphTallyCollector", "1", address(controller), 0);

        // 7. RPCDataService (impl + proxy) — owner = deployer for the addChain call below.
        RPCDataService serviceImpl = new RPCDataService(address(controller), address(tallyCollector));
        RPCDataService service = RPCDataService(address(new ERC1967Proxy(
            address(serviceImpl),
            abi.encodeCall(RPCDataService.initialize, (deployer, pauseGuardian))
        )));

        // 8. Provision stake for the provider in the mock staking contract.
        staking.setProvision(provider, address(service), SUFFICIENT_PROVISION, SUFFICIENT_THAWING);

        // 9. Enable chain 31337 (tier 0 = standard).
        service.addChain(CHAIN_ID, 0);

        vm.stopBroadcast();

        // ===============================================================
        // Phase 2: provider registers and starts the service
        // ===============================================================
        vm.startBroadcast(PROVIDER_KEY);
        service.register(provider, abi.encode("http://127.0.0.1:7700", "u1hx", paymentWallet));
        service.startService(provider, abi.encode(uint64(CHAIN_ID), uint8(0), "http://127.0.0.1:7700"));
        vm.stopBroadcast();

        // ===============================================================
        // Phase 3: gateway authorises its signer in GraphTallyCollector
        // ===============================================================
        {
            uint256 proofDeadline = block.timestamp + 1 days;
            bytes32 messageHash = keccak256(
                abi.encodePacked(block.chainid, address(tallyCollector), "authorizeSignerProof", proofDeadline, gateway)
            );
            bytes32 digest = MessageHashUtils.toEthSignedMessageHash(messageHash);
            (uint8 v, bytes32 r, bytes32 s) = vm.sign(GATEWAY_SIGNER_KEY, digest);
            bytes memory proof = abi.encodePacked(r, s, v);

            vm.startBroadcast(GATEWAY_KEY);
            tallyCollector.authorizeSigner(gatewaySigner, proofDeadline, proof);

            // ===============================================================
            // Phase 4: gateway funds the escrow bucket for this provider
            // ===============================================================
            grt.mint(gateway, DEPOSIT);
            grt.approve(address(escrow), DEPOSIT);
            escrow.deposit(address(tallyCollector), provider, DEPOSIT);

            // Also fund escrow keyed by the gatewaySigner address so that the
            // e2e service (which uses validated.signer as the escrow payer) can
            // verify balance for gateway-signed receipts.
            grt.mint(gateway, DEPOSIT);
            grt.approve(address(escrow), DEPOSIT);
            escrow.depositTo(gatewaySigner, address(tallyCollector), provider, DEPOSIT);
            vm.stopBroadcast();
        }

        // ===============================================================
        // Phase 5: write the fixture JSON
        // ===============================================================
        string memory json = string(
            abi.encodePacked(
                '{"rpcDataService":"',          vm.toString(address(service)),         '",',
                '"graphTallyCollector":"',      vm.toString(address(tallyCollector)),  '",',
                '"paymentsEscrow":"',           vm.toString(address(escrow)),          '",',
                '"grtToken":"',                 vm.toString(address(grt)),             '",',
                '"providerAddress":"',          vm.toString(provider),                 '",',
                '"providerKey":"0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",',
                '"gatewayAddress":"',           vm.toString(gateway),                  '",',
                '"gatewayKey":"0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",',
                '"gatewaySignerAddress":"',     vm.toString(gatewaySigner),            '",',
                '"gatewaySignerKey":"0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6",',
                '"paymentWallet":"',            vm.toString(paymentWallet),            '"}'
            )
        );
        vm.writeFile("out/e2e-fixture.json", json);
    }
}
