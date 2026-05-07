// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.27;
import {Script, console2} from "forge-std/Script.sol";

interface IGraphTallyCollector {
    struct ReceiptAggregateVoucher {
        bytes32 collectionId;
        address payer;
        address serviceProvider;
        address dataService;
        uint64 timestampNs;
        uint128 valueAggregate;
        bytes metadata;
    }

    struct SignedRAV {
        ReceiptAggregateVoucher rav;
        bytes signature;
    }
}

interface IRPCDataService {
    function collect(address serviceProvider, uint8 paymentType, bytes calldata data) external returns (uint256 fees);
    function isRegistered(address) external view returns (bool);
    function paymentsDestination(address) external view returns (address);
}

contract TestCollect is Script {
    address constant RPC_DS = 0x73846272813065c3e4Efdb3Fb82E0d128c8C2364;
    address constant PROVIDER = 0xb43B2CCCceadA5292732a8C58ae134AdEFcE09Bb;
    address constant OPERATOR = 0xd370EE7A865779D252F65c7455592f9f7d6F9A99;

    function run() external {
        console2.log("provider registered:", IRPCDataService(RPC_DS).isRegistered(PROVIDER));
        console2.log("payments dest:", IRPCDataService(RPC_DS).paymentsDestination(PROVIDER));

        IGraphTallyCollector.SignedRAV memory signedRav = IGraphTallyCollector.SignedRAV({
            rav: IGraphTallyCollector.ReceiptAggregateVoucher({
                collectionId: 0x9c901c08bbcbee383e781487674d7123e150e7e1b78b521db6d4a71066607f46,
                payer: 0x7D14ae5f20cc2f6421317386Aa8E79e8728353d9,
                serviceProvider: PROVIDER,
                dataService: RPC_DS,
                timestampNs: 1776352063069443063,
                valueAggregate: 4000000000000,
                metadata: bytes("")
            }),
            signature: hex"36ddba6acaaf5852f298e4cccf745b629956985f8b86cfe9b442a321e444fc1d17c3e0fec1286fef7c16b92c6a3599a0683000db95d75da02439794a2e44b9011c"
        });

        bytes memory data = abi.encode(signedRav, uint256(0));

        vm.startBroadcast(OPERATOR);
        (bool ok, bytes memory ret) =
            RPC_DS.call(abi.encodeWithSignature("collect(address,uint8,bytes)", PROVIDER, uint8(0), data));
        console2.log("collect ok:", ok);
        if (!ok) {
            console2.logBytes(ret);
            if (ret.length >= 4) {
                bytes4 sel;
                assembly { sel := mload(add(ret, 32)) }
                console2.logBytes4(sel);
            }
        } else {
            uint256 fees = abi.decode(ret, (uint256));
            console2.log("fees collected:", fees);
        }
        vm.stopBroadcast();
    }
}
