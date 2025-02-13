// SPDX-License-Identifier: MIT

pragma solidity ^0.8.24;

import "./GwynethData.sol";

contract ExtensionOracle {
    // TODO(Brecht): change to transient
    uint private returndataCounter;
    GwynethData.ReturnData[] private returndata;

    address private constant gwyneth = 0x9fCF7D13d10dEdF17d0f24C62f0cf4ED462f65b7;

    fallback() external payable {
        _returnData();
    }

    receive() external payable {
       _returnData();
    }

    function _returnData() internal {
        if (msg.sender == gwyneth) {
            returndataCounter = 0;
            returndata = abi.decode(msg.data, (GwynethData.ReturnData[]));
            assembly {
                tstore(0, 1)
            }
        } else {
            //require(returndataCounter < returndata.length, "invalid call pattern");

            uint initialized;
            assembly {
                initialized := tload(0)
            }

            // Allow forge simulation to work
            if (initialized == 0 || returndataCounter >= returndata.length) {
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

            GwynethData.ReturnData memory returnData = returndata[returndataCounter++];
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