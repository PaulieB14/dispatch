// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.27;

import {Script, console2} from "forge-std/Script.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import {RPCDataService} from "../src/RPCDataService.sol";

/// @notice Deploy RPCDataService (UUPS upgradeable) to a target network.
///
/// Deploys the implementation contract and an ERC1967Proxy pointing to it,
/// calling initialize() atomically via the proxy constructor.
///
/// Usage (Arbitrum One mainnet):
///   forge script script/Deploy.s.sol \
///     --rpc-url arbitrum_one \
///     --private-key $PRIVATE_KEY \
///     --broadcast \
///     --verify \
///     -vvvv
///
/// Usage (Arbitrum Sepolia testnet):
///   forge script script/Deploy.s.sol \
///     --rpc-url arbitrum_sepolia \
///     --private-key $PRIVATE_KEY \
///     --broadcast \
///     --verify \
///     -vvvv
///
/// Required env vars (see .env.example):
///   PRIVATE_KEY           — deployer private key (hex, 0x-prefixed)
///   OWNER                 — governance multisig or deployer address
///   GRAPH_CONTROLLER      — Graph Protocol Controller address
///   GRAPH_TALLY_COLLECTOR — GraphTallyCollector contract address
///   GRT_TOKEN             — GRT ERC-20 token address
///   PAUSE_GUARDIAN        — address authorised to pause the service
///
/// Horizon addresses — Arbitrum One (42161, mainnet):
///   Controller:           cast call 0xb2Bb92d0DE618878E438b55D5846cfecD9301105 "controller()(address)" --rpc-url arbitrum_one
///   HorizonStaking:       0x00669A4CF01450B64E8A2A20E9b1FCB71E61eF03
///   GraphTallyCollector:  0x8f69F5C07477Ac46FBc491B1E6D91E2bb0111A9e
///   PaymentsEscrow:       0xf6Fcc27aAf1fcD8B254498c9794451d82afC673E
///   GRT Token:            0x9623063377AD1B27544C965cCd7342f7EA7e88C7
///
/// Horizon addresses — Arbitrum Sepolia (421614, testnet):
///   Controller:           0x9DB3ee191681f092607035d9BDA6e59FbEaCa695
///   HorizonStaking:       0xFf2Ee30de92F276018642A59Fb7Be95b3F9088Af
///   GraphTallyCollector:  0xacC71844EF6beEF70106ABe6E51013189A1f3738
///   PaymentsEscrow:       0x09B985a2042848A08bA59060EaF0f07c6F5D4d54
contract Deploy is Script {
    /// Phase 1 supported chains and their minimum provisions (in GRT wei).
    struct ChainInit {
        uint256 chainId;
        uint256 minProvisionTokens; // 0 = use DEFAULT_MIN_PROVISION
    }

    function run() external {
        address owner_ = vm.envAddress("OWNER");
        address controller = vm.envAddress("GRAPH_CONTROLLER");
        address graphTallyCollector = vm.envAddress("GRAPH_TALLY_COLLECTOR");
        address pauseGuardian = vm.envAddress("PAUSE_GUARDIAN");

        ChainInit[] memory chains = _phase1Chains();

        vm.startBroadcast();

        // Deploy implementation (immutables set here, initializers disabled).
        RPCDataService impl = new RPCDataService(controller, graphTallyCollector);
        console2.log("RPCDataService implementation:", address(impl));

        // Deploy proxy — initialize() is called atomically in the constructor.
        bytes memory initData = abi.encodeCall(RPCDataService.initialize, (owner_, pauseGuardian));
        ERC1967Proxy proxy = new ERC1967Proxy(address(impl), initData);
        RPCDataService service = RPCDataService(address(proxy));
        console2.log("RPCDataService proxy deployed at:", address(service));

        for (uint256 i = 0; i < chains.length; i++) {
            service.addChain(chains[i].chainId, chains[i].minProvisionTokens);
            console2.log("  Added chain:", chains[i].chainId);
        }

        vm.stopBroadcast();

        // Persist addresses for downstream scripts
        console2.log("\nAdd to your .env:");
        console2.log("RPC_DATA_SERVICE_ADDRESS=", vm.toString(address(proxy)));
        console2.log("RPC_DATA_SERVICE_IMPL=", vm.toString(address(impl)));
    }

    function _phase1Chains() internal pure returns (ChainInit[] memory chains) {
        chains = new ChainInit[](10);
        chains[0] = ChainInit({chainId: 1, minProvisionTokens: 0}); // Ethereum mainnet
        chains[1] = ChainInit({chainId: 42161, minProvisionTokens: 0}); // Arbitrum One
        chains[2] = ChainInit({chainId: 10, minProvisionTokens: 0}); // Optimism
        chains[3] = ChainInit({chainId: 8453, minProvisionTokens: 0}); // Base
        chains[4] = ChainInit({chainId: 137, minProvisionTokens: 0}); // Polygon PoS
        chains[5] = ChainInit({chainId: 56, minProvisionTokens: 0}); // BNB Chain
        chains[6] = ChainInit({chainId: 43114, minProvisionTokens: 0}); // Avalanche C-Chain
        chains[7] = ChainInit({chainId: 324, minProvisionTokens: 0}); // zkSync Era
        chains[8] = ChainInit({chainId: 59144, minProvisionTokens: 0}); // Linea
        chains[9] = ChainInit({chainId: 534352, minProvisionTokens: 0}); // Scroll
    }
}
