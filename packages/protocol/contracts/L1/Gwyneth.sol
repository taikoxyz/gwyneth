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

    event Executed(address to, uint256 value, bytes data, bool success, bytes result);

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

            (bool success, bytes memory result) = payable(_tx.addr).call{value: _tx.value}(_tx.data);
            emit Executed(_tx.addr, _tx.value, _tx.data, success, result);

            if (!success) {
                errorOut(result);
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

    // This contract stores all ETH on L2
    receive() external payable {}
}
