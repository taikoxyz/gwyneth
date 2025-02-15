// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;


import "../L1/GwynethData.sol";

contract GwynethContract {
    function applyStateDelta(GwynethData.StateDiffAccount calldata accountChanges)
        external
        payable
    {
        //require(msg.sender == gwyneth, "not from gwyneth contract");
        // Run over all state changes
        for (uint256 i = 0; i < accountChanges.storageSlots.length; i++) {
            // Apply the updated state to the storage
            bytes32 key = accountChanges.storageSlots[i].key;
            bytes32 value = accountChanges.storageSlots[i].value;
            // Possible to check the slot against any variable.slot
            // to e.g. throw a custom event
            assembly {
                sstore(key, value)
            }
        }

        if (accountChanges.balanceChange > 0) {
            (bool success, ) = msg.sender.call{value: accountChanges.balanceChange }("");
            require(success, "Failed to send Ether");
        }
    }
}