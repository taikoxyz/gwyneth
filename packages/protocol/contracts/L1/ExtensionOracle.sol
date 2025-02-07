// SPDX-License-Identifier: MIT

pragma solidity ^0.8.24;

import "./GwynethData.sol";

contract ExtensionOracle {
    // TODO(Brecht): change to transient
    uint private returndataCounter;
    GwynethData.ReturnData[] private returndata;

    address private constant gwyneth = 0x9f5eaC3d8e082f47631F1551F1343F23cd427162;

    fallback() external payable {
        _returnData();
    }

    receive() external payable {
       _returnData();
    }

    function _returnData() internal {
        if (msg.sender == gwyneth) {
            // returndata = abi.decode(msg.data, (GwynethData.ReturnData[]));
        } else {
            //require(returndataCounter < returndata.length, "invalid call pattern");

            // if (returndataCounter >= returndata.length) {
            //     return;
            // }

            // GwynethData.ReturnData memory returnData = returndata[returndataCounter++];
            // bytes memory data = returnData.data;
            // if (returnData.isRevert) {
            //     assembly {
            //         revert(add(data, 32), mload(data))
            //     }
            // } else {
            //     assembly {
            //         return(add(data, 32), mload(data))
            //     }
            // }
        }
    }
}