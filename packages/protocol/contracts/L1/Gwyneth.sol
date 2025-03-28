// SPDX-License-Identifier: MIT

pragma solidity ^0.8.24;

import "./IGwyneth.sol";
import "./GwynethData.sol";
import "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";


/// @title Gwyneth
contract Gwyneth is IGwyneth {
    address public owner;
    // We don't really need to store this but let's do it anyway for now
    bytes32 public ultraHash;
    // Temporary proposer map
    mapping(address proposer => bool whitelisted) public proposers;

    /// @dev Emitted when a block is proposed.
    /// @param block The block metadata containing information about the proposed
    /// block.
    event BlockProposed(GwynethData.UltraBlock block);

    event Executed(address to, uint256 value, bytes data, bool success, bytes result, uint gas);

    /// @notice Initializes the rollup.
    /// @param _genesisUltraHash The hash of the genesis ultra block.
    function init(
        address _owner,
        bytes32 _genesisUltraHash
    )
        external
    {
        owner = _owner;
        ultraHash = _genesisUltraHash;
        proposers[0xE25583099BA105D9ec0A67f5Ae86D90e50036425] = true;
    }

    function propose(GwynethData.UltraBlock calldata _block, GwynethData.Proof calldata proof)
        external
        payable
        override
    {
        require(_block.parentL1BlockHash == blockhash(block.number - 1), "included in an unexpected L1 block");
        //require(_block.parentUltraHash == ultraHash, "parent ULTRA hash mismatch");

        // for (uint i = 0; i < _block.blobHashes.length; i++) {
        //     require(blobhash(i) == _block.blobHashes[i], "unexpected blob hash");
        // }

        for (uint i = 0; i < _block.blocks.length; i++) {
            _propose(_block.blocks[i]);
        }
        _prove(_block, proof);

        ultraHash = _block.ultraHash;

        emit BlockProposed({ block: _block });
    }

    function _propose(GwynethData.Block calldata _block)
        private
    {
        // Apply L1 state updates
        for (uint i = 0; i < _block.l1Block.transactions.length; i++) {
            GwynethData.Transaction calldata _tx = _block.l1Block.transactions[i];

            (bool success, bytes memory result) = payable(_tx.addr).call{value: _tx.value, gas: _tx.gas }(_tx.data);
            emit Executed(_tx.addr, _tx.value, _tx.data, success, result,  _tx.gas);

            // if (!_tx.reverts && !success) {
            //     assembly {
            //         revert(add(result, 32), mload(result))
            //     }
            // }
        }
    }

    function _prove(GwynethData.UltraBlock calldata _block, GwynethData.Proof calldata proof)
        view
        private
    {
        bytes32 inputHash = keccak256(abi.encode(_block));
        require(proposers[ECDSA.recover(inputHash, proof.proof)] == true, "invalid proof");
    }

    // This contract stores all L2 ETH
    receive() external payable {}


    function setProposer(address proposer, bool whitelisted)
        external
    {
        require(msg.sender == owner);
        proposers[proposer] = whitelisted;
    }
}
