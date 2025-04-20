// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import "forge-std/Test.sol";
import "forge-std/Script.sol";
import "forge-std/console2.sol";

import "../contracts/examples/xERC20.sol";
import "../contracts/examples/EVM.sol";

contract XSetup is Script {
    uint constant chainIdParent = 160010;
    uint constant chainIdA = 167010;
    uint constant chainIdB = 167011;

    address constant ALICE = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266; //Can stay as is - test values anyways
    address constant BOB = 0xE25583099BA105D9ec0A67f5Ae86D90e50036425; //Can stay as is - test values anyways
    address constant CHARLIE = 0x614561D2d143621E126e87831AEF287678B442b8; //Can stay as is - test values anyways
    uint256 constant ALICE_PK = 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80;//Can stay as is - test values anyways
    uint256 constant BOB_PK = 0x39725efee3fb28614de3bacaffe4cc4bd8c436257e2c8bb887c4b5c4be45e76d;//Can stay as is - test values anyways
    uint256 constant CHARLIE_PK = 0x53321db7c1e331d93a11a41d16f004d7ff63972ec8ec7c25db329728ceeb1710;//Can stay as is - test values anyways

    xERC20 public token;

    using EVM for address;
    using EVM for address payable;

    function ChainAddress(uint256 chainId, XSetup contractAddr) internal view returns (XSetup) {
        return XSetup(address(contractAddr).onChain(chainId));
    }

    function on(uint256 chainId) internal view returns (XSetup) {
        return ChainAddress(chainId, this);
    }

    function setUp() public {}

    function run() public {
        vm.startBroadcast(0xbcdf20249abf0ed6d944c0288fad489e33f66b3960d9e6229c1cd214ed3bbe31);

        (bool success, ) = ALICE.call{value: 10 ether}("");
        require(success, "Failed to send Ether");

        vm.stopBroadcast();

        // vm.startBroadcast(ALICE_PK);

        // token = new xERC20(99_999);

        // console2.log("xERC20 address is:", address(token));

        // vm.stopBroadcast();

        // // require(token.balanceOf(ALICE) == 99_999);

        // // vm.startBroadcast(ALICE_PK);

        // // token.xTransfer(chainIdParent, BOB, 666);

        // // vm.stopBroadcast();

        // on(chainIdParent).runParent();
        // on(chainIdA).runA();
        // on(chainIdB).runB();
    }

    function runParent() public {
        //uint balance_before = on(chainIdB).getETHBalance(ALICE);

        vm.startBroadcast(ALICE_PK);
        // L1 -> L2 (ETH)
        token.sendETH{value: 4 ether}(chainIdB, payable(ALICE));
        vm.stopBroadcast();

        //uint balance_after = on(chainIdB).getETHBalance(ALICE);
        //require(balance_before + 4 ether == balance_after);

        vm.startBroadcast(ALICE_PK);
        // L1 -> L2 (ERC20)
        token.xTransfer(chainIdA, BOB, 333);
        token.xTransfer(chainIdB, CHARLIE, 666);
        token.xTransfer(chainIdParent, chainIdB, CHARLIE, 666);
        vm.stopBroadcast();

        require((xERC20(address(token).onChain(chainIdA))).balanceOf(BOB) == 333);
        require((xERC20(address(token).onChain(chainIdB))).balanceOf(CHARLIE) == 666 + 666);
    }

    function runA() public {
        xERC20 token = on(chainIdParent).token();

        require(block.chainid == chainIdA);

        vm.startBroadcast(ALICE_PK);

        // Deposit 999 tokens to L2A (L2 -> L1 -> L2)
        token.xTransfer(chainIdParent, chainIdA, ALICE, 999);

        // Transfer 666 tokens to Bob on L2B (chainId: 167011)
        token.xTransfer(chainIdB, BOB, 666);

        // Transfer some ETH to L2B
        token.sendETH{value: 3.77 ether}(chainIdB, payable(CHARLIE));

        vm.stopBroadcast();
    }

    function runB() public {
        xERC20 token = on(chainIdParent).token();

        require(block.chainid == chainIdB);

        vm.startBroadcast(BOB_PK);

        // Withdraw some tokens to L1
        token.xTransfer(chainIdParent, CHARLIE, 222);

        // L2 -> L1 (ETH)
        token.sendETH{value: 1.11 ether}(chainIdParent, payable(BOB));

        vm.stopBroadcast();
    }

    function getETHBalance(address addr) public returns (uint) {
       return addr.balance;
    }
}