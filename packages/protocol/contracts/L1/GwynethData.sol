// SPDX-License-Identifier: MIT

pragma solidity ^0.8.20;

/// @title GwynethData
/// @notice This library defines various data structures used in the Gwyneth
/// protocol.
library GwynethData {
    /// @dev ULTRA TX
    struct UltraBlock {
        bytes32 ultraHash;
        bytes32 parentUltraHash;
        bytes32 parentL1BlockHash;

        bytes32[] blobHashes;
        bytes da;

        Block[] blocks;
    }

    /// @dev Struct containing all block data
    struct Block {
        L1Block l1Block;

        bytes32 extraData;
        address coinbase;

        uint24 daByteOffset;
        uint24 daByteSize;
    }

    /// @dev Struct representing the state that has to be applied to L1 in sequential order
    struct L1Block {
        Transaction[] transactions;
    }

    struct Transaction {
        address addr;
        bytes data;
        uint256 value;
        uint64 gas;
        bool reverts;
    }

    struct StateDiffAccount {
        StateDiffStorageSlot[] storageSlots;
        uint balanceChange;
    }

    struct StateDiffStorageSlot {
        bytes32 key;
        bytes32 value;
    }

    struct Proof {
        bytes proof;
    }
}
