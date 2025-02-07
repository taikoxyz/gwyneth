// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.24;

import "./preconfs/ISequencerRegistry.sol";
import "../gwyneth/GwynethContract.sol";
import "./GwynethData.sol";
import "./ExtensionOracle.sol";

import "../examples/DelegateContract.sol";

/// @title Gwyneth
contract Gwyneth {
    address public owner;

    ExtensionOracle public extensionOracle = ExtensionOracle(payable(0x1ADB9959EB142bE128E6dfEcc8D571f07cd66DeE));

    /// @dev Emitted when a block is proposed.
    /// @param blockId The ID of the proposed block.
    /// @param meta The block metadata containing information about the proposed
    /// block.
    event BlockProposed(uint256 indexed blockId, GwynethData.BlockMetadata meta);

    event Executed(address to, uint256 value, bytes data);

    /// @notice Initializes the rollup.
    /// @param _addressManager The {AddressManager} address.
    /// @param _genesisBlockHash The block hash of the genesis block.
    function init(
        address _owner,
        address _addressManager,
        bytes32 _genesisBlockHash
    )
        external
    {
        owner = _owner;
    }

    /// @dev Proposes multiple blocks
    function proposeBlock(GwynethData.BlockMetadata[] calldata blocks)
        external
        payable
    {
        for (uint i = 0; i < blocks.length; i++) {
            _proposeBlock(blocks[i]);
        }
        _prove(blocks);
    }

    function _proposeBlock(GwynethData.BlockMetadata calldata _block)
        private
    {
        require(_block.parentBlockHash == blockhash(block.number - 1), "included in an unexpected L1 block (hash)");
        require(_block.timestamp == block.timestamp, "included in an unexpected L1 block (timestamp)");

        // Apply L1 state updates
        for (uint i = 0; i < _block.l1Block.transactions.length; i++) {
            GwynethData.Transaction calldata _tx = _block.l1Block.transactions[i];
            for (uint j = 0; j < _tx.calls.length; j++) {
                GwynethData.Call calldata call = _tx.calls[j];

                // Set return data
                if (call.returnData.length > 0) {
                    (bool success, bytes memory result) = address(extensionOracle).call(abi.encode(call.returnData));
                    require(success == true, "call to extension oracle failed");
                }

                //DelegateContract.Call[] memory calls = new DelegateContract.Call[](0);
                //DelegateContract(payable(_tx.addr)).execute(calls);
                (bool success, bytes memory result) = _tx.addr.call{value: call.value}(call.data);
                if (!success) {
                    errorOut(result);
                }
            }
            if (_tx.slots.length > 0) {
                GwynethContract(_tx.addr).applyStateDelta(_tx.slots);
            }
        }

        emit BlockProposed({ blockId: _block.l2BlockNumber, meta: _block });
    }

    function _prove(GwynethData.BlockMetadata[] calldata _block)
        private
    {

    }

    function errorOut(bytes memory result)
        private
    {
        assembly ("memory-safe") {
            revert(add(result, 32), mload(result))
        }
    }
}
