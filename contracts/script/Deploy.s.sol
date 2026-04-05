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
/// Required env vars:
///   PRIVATE_KEY           — deployer private key
///   GRAPH_CONTROLLER      — Graph Protocol Controller address (from GraphDirectory)
///   PAUSE_GUARDIAN        — address authorised to pause the service
///
/// Arbitrum Sepolia addresses (for reference):
///   HorizonStaking:       0x865365C425f3A593Ffe698D9c4E6707D14d51e08
///   GraphTallyCollector:  0x382863e7B662027117449bd2c49285582bbBd21B
///   PaymentsEscrow:       0x1e4dC4f9F95E102635D8F7ED71c5CdbFa20e2d02
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

        RPCDataService service = new RPCDataService(owner_, controller, graphTallyCollector, pauseGuardian);
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
        chains = new ChainInit[](4);
        chains[0] = ChainInit({chainId: 1,     minProvisionTokens: 0});      // Ethereum mainnet
        chains[1] = ChainInit({chainId: 42161, minProvisionTokens: 0});      // Arbitrum One
        chains[2] = ChainInit({chainId: 10,    minProvisionTokens: 0});      // Optimism
        chains[3] = ChainInit({chainId: 8453,  minProvisionTokens: 0});      // Base
    }
}
