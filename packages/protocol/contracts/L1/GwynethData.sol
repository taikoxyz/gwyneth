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
        // todo: Do we need this below ?
        // bytes32 blobId OR blobHash; ? as per in current taiko-mono's preconfirmation branch ?
        bool blobUsed;
        bytes txList;
        bytes stateDiffs;
        StateDiff l1StateDiff;
    }

    /// @dev Struct representing the state delta that has to be applied to L1
    struct StateDiff {
        StateDiffAccount[] accounts;
    }

    struct StateDiffAccount {
        address addr;
        StateDiffStorageSlot[] slots;
    }

    struct StateDiffStorageSlot {
        bytes32 key;
        bytes32 value;
    }
}
