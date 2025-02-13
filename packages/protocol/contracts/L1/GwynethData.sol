// SPDX-License-Identifier: MIT

pragma solidity ^0.8.20;

/// @title GwynethData
/// @notice This library defines various data structures used in the Gwyneth
/// protocol.
library GwynethData {
    /// @dev Struct containing data only required for proving a block
    struct BlockMetadata {
        bytes32 blockHash;
        bytes32 parentBlockHash;
        bytes32 parentMetaHash;
        bytes32 l1Hash;
        uint256 difficulty;
        bytes32 blobHash;
        bytes32 extraData;
        address coinbase;
        uint64 l2BlockNumber;
        uint32 gasLimit;
        uint32 l1StateBlockNumber;
        uint64 timestamp;
        uint24 txListByteOffset;
        uint24 txListByteSize;
        bool blobUsed;
        bytes txList;
        bytes stateDiffs;
        L1Block l1Block;
    }

    /// @dev Struct representing the state that has to be applied to L1 in sequential order
    struct L1Block {
        Transaction[] transactions;
    }

    struct Transaction {
        address addr;
        bytes data;
        uint256 value;
    }

    struct StateDiffStorageSlot {
        bytes32 key;
        bytes32 value;
    }

    struct ReturnData {
        bytes data;
        bool isRevert;
    }
}
