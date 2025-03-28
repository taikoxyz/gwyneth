// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import "forge-std/Script.sol";
import "forge-std/console2.sol";

contract XSetup is Script {
    address ALICE = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;

    function setUp() public {}

    function run() public {
        vm.startBroadcast(0xbcdf20249abf0ed6d944c0288fad489e33f66b3960d9e6229c1cd214ed3bbe31);

        (bool success, ) = ALICE.call{value: 10 ether}("");
        require(success, "Failed to send Ether");

        vm.stopBroadcast();
    }
}