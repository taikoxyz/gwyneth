// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import "./GwynethData.sol";

/// @title IGwyneth
/// @custom:security-contact security@taiko.xyz
interface IGwyneth {
    /// @notice Proposes a Gwyneth block
    function propose(GwynethData.UltraBlock calldata _block, GwynethData.Proof calldata proof)
        external
        payable;
}
