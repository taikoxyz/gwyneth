// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "./GwynethData.sol";

/// @title IGwyneth
/// @custom:security-contact security@taiko.xyz
interface IGwyneth {
    /// @notice Proposes Gwyneth blocks
    function proposeBlock(GwynethData.BlockMetadata[] calldata data)
        external
        payable;
}
