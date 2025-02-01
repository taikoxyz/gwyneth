// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "./preconfs/ISequencerRegistry.sol";
import "../gwyneth/GwynethContract.sol";
import "./GwynethData.sol";

/// @title Gwyneth
contract Gwyneth {

    /// @dev Emitted when a block is proposed.
    /// @param blockId The ID of the proposed block.
    /// @param meta The block metadata containing information about the proposed
    /// block.
    event BlockProposed(uint256 indexed blockId, GwynethData.BlockMetadata meta);

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

    }

    /// @dev Proposes multiple blocks
    function proposeBlock(GwynethData.BlockMetadata[] calldata data)
        external
        payable
    {
        for (uint256 i = 0; i < data.length; i++) {
            _proposeBlock(data[i]);
        }
    }

    /// Proposes a Taiko L2 block.
    /// @param _block Block parameters, currently an encoded BlockMetadata object.
    function _proposeBlock(GwynethData.BlockMetadata calldata _block)
        private
    {
        require(_block.timestamp == block.timestamp, "included in an unexpected L1 block");
        require(_block.parentBlockHash == blockhash(block.number - 1), "included in an unexpected L1 block (hash)");

        // Apply L1 state updates
        for (uint i = 0; i < _block.l1StateDiff.accounts.length; i++) {
            GwynethContract(_block.l1StateDiff.accounts[i].addr).applyStateDelta(_block.l1StateDiff.accounts[i].slots);
        }

        emit BlockProposed({ blockId: _block.l2BlockNumber, meta: _block });
    }
}
