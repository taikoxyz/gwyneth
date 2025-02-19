// SPDX-License-Identifier: MIT

pragma solidity ^0.8.28;

import "./GwynethData.sol";

contract ExtensionOracle {

    struct ReturnData {
        bytes data;
        bool isRevert;
    }

    uint private transient returndataCounter;
    // TODO(Brecht): change to transient, solidity doesn't support this yet
    ReturnData[] private returndata;

    address payable private constant gwyneth = payable(0x9fCF7D13d10dEdF17d0f24C62f0cf4ED462f65b7);

    fallback() external payable {
        _returnData();
    }

    receive() external payable {
       _returnData();
    }

    function _returnData() internal {
        if (msg.sender == gwyneth) {
            returndata = abi.decode(msg.data, (ReturnData[]));
        } else {
            //require(returndataCounter < returndata.length, "invalid call pattern");

            // Allow forge simulation to work
            if (returndataCounter >= returndata.length) {
                (bool success, bytes memory data) = msg.sender.call(msg.data);
                if (!success) {
                    assembly {
                        revert(add(data, 32), mload(data))
                    }
                } else {
                    assembly {
                        return(add(data, 32), mload(data))
                    }
                }
            }

            // Collect all ETH in the Gwyneth contract
            if (msg.value > 0) {
                (bool success, ) = gwyneth.call{value: msg.value }("");
                require(success, "Failed to send Ether");
            }

            ReturnData memory returnData = returndata[returndataCounter++];
            bytes memory data = returnData.data;
            if (returnData.isRevert) {
                assembly {
                    revert(add(data, 32), mload(data))
                }
            } else {
                assembly {
                    return(add(data, 32), mload(data))
                }
            }
        }
    }
}