// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.27;

import {Script, console2} from "forge-std/Script.sol";
import {RPCDataService} from "../src/RPCDataService.sol";

/// @notice Deploy RPCDataService to a target network.
///
/// Usage (Arbitrum Sepolia testnet):
///   forge script script/Deploy.s.sol \
///     --rpc-url arbitrum_sepolia \
///     --broadcast \
///     --verify \
///     -vvvv
///
/// Required env vars (see .env.example):
///   PRIVATE_KEY           — deployer private key (hex, 0x-prefixed)
///   OWNER                 — governance multisig or deployer address
///   GRAPH_CONTROLLER      — Graph Protocol Controller address
///   GRAPH_TALLY_COLLECTOR — GraphTallyCollector contract address
///   PAUSE_GUARDIAN        — address authorised to pause the service
///
/// Horizon addresses — Arbitrum Sepolia (421614):
///   Controller:           0x9DB3ee191681f092607035d9BDA6e59FbEaCa695
///   HorizonStaking:       0xFf2Ee30de92F276018642A59Fb7Be95b3F9088Af
///   GraphTallyCollector:  0xacC71844EF6beEF70106ABe6E51013189A1f3738
///   PaymentsEscrow:       0x09B985a2042848A08bA59060EaF0f07c6F5D4d54
///
/// Horizon addresses — Arbitrum One (42161):
///   GraphTallyCollector:  0x8f69F5C07477Ac46FBc491B1E6D91E2be0111A9e
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

        address grtToken = vm.envAddress("GRT_TOKEN");
        RPCDataService service = new RPCDataService(owner_, controller, graphTallyCollector, pauseGuardian, grtToken);
        console2.log("RPCDataService deployed at:", address(service));

        for (uint256 i = 0; i < chains.length; i++) {
            service.addChain(chains[i].chainId, chains[i].minProvisionTokens);
            console2.log("  Added chain:", chains[i].chainId);
        }

        vm.stopBroadcast();

        // Persist address for downstream scripts
        console2.log("\nAdd to your .env:");
        console2.log("RPC_DATA_SERVICE_ADDRESS=", vm.toString(address(service)));
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
