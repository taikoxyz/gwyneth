// SPDX-License-Identifier: MIT

pragma solidity >=0.8.12 <0.9.0;

import "./xERC20.sol";

contract DelegateContract {
    event Executed(address indexed to, uint256 value, bytes data);

    address constant TOKEN_ADDRESS = 0x5FbDB2315678afecb367f032d93F642f64180aa3;
    address BOB = 0xE25583099BA105D9ec0A67f5Ae86D90e50036425; //Can stay as is - test values anyways

    struct Call {
        bytes data;
        address to;
        uint256 value;
    }

    function execute(Call[] memory calls) external payable {
        // for (uint256 i = 0; i < calls.length; i++) {
        //     Call memory call = calls[i];
        //     (bool success, bytes memory result) = call.to.call{value: call.value}(call.data);
        //     require(success, string(result));
        //     emit Executed(call.to, call.value, call.data);
        // }
        xERC20(TOKEN_ADDRESS).xTransfer(167010, BOB, 333);
    }

    receive() external payable {}
}