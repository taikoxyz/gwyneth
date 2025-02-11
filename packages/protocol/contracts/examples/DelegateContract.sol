// SPDX-License-Identifier: MIT

pragma solidity >=0.8.12 <0.9.0;

import "./xERC20.sol";

contract DelegateContract {
    event Executed(address indexed to, uint256 value, bytes data);

    struct SubCall {
        bytes data;
        address to;
        uint256 value;
    }

    function execute(SubCall[] memory calls) external payable {
        for (uint256 i = 0; i < calls.length; i++) {
            SubCall memory call = calls[i];
            (bool success, bytes memory result) = call.to.call{value: call.value}(call.data);
            require(success, string(result));
            emit Executed(call.to, call.value, call.data);
        }
    }

    receive() external payable {}
}