// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import "forge-std/Test.sol";
import "forge-std/Script.sol";
import "forge-std/console2.sol";

import "../contracts/examples/xERC20.sol";
import "../contracts/examples/EVM.sol";

contract XDeployAndTest is Script {
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

    function ChainAddress(uint256 chainId, XDeployAndTest contractAddr) internal view returns (XDeployAndTest) {
        return XDeployAndTest(address(contractAddr).onChain(chainId));
    }

    function on(uint256 chainId) internal view returns (XDeployAndTest) {
        return ChainAddress(chainId, this);
    }

    function setUp() public {}

    function run() public {
        vm.startBroadcast(0xbcdf20249abf0ed6d944c0288fad489e33f66b3960d9e6229c1cd214ed3bbe31);

        (bool success, ) = ALICE.call{value: 10 ether}("");
        require(success, "Failed to send Ether");

        vm.stopBroadcast();

        vm.startBroadcast(ALICE_PK);

        token = new xERC20(99_999);

        console2.log("xERC20 address is:", address(token));

        vm.stopBroadcast();

        require(block.chainid == chainIdParent);

        vm.startBroadcast(ALICE_PK);

        token.xTransfer(chainIdParent, BOB, 666);

        vm.stopBroadcast();

        // L1 -> L2 (ETH)
        transferETHAndCheck(ALICE.on(chainIdParent), ALICE.on(chainIdB), 4 ether);

        // L1 -> L2 (ERC20)
        transferAndCheck(ALICE.on(chainIdParent), BOB.on(chainIdA), 333);
        transferAndCheck(ALICE.on(chainIdParent), CHARLIE.on(chainIdB), 666);
        transferAndCheck(ALICE.on(chainIdParent), CHARLIE.on(chainIdB), 666);

        // Transfer 666 tokens to Bob on L2B
        transferAndCheck(ALICE.on(chainIdParent), BOB.on(chainIdB), 666);

        on(chainIdA).runA();
        on(chainIdB).runB();

        // Transfer some ETH to L2B
        transferETHAndCheck(ALICE.on(chainIdA), CHARLIE.on(chainIdB), 3.77 ether);

        // Withdraw some ETH on L2B to L1
        transferETHAndCheck(ALICE.on(chainIdB), BOB.on(chainIdParent), 1.11 ether);

        // Verify that the chain ids are updated correctly
        vm.startBroadcast(ALICE_PK);
        checkChainIds();
        vm.stopBroadcast();
    }

    function runA() public {
        // Check if the block chain id has been updated
        require(block.chainid == chainIdA);

        // Deposit 999 tokens to L2A (L2A -> L1 -> L2A)
        transferAndCheck(ALICE.on(chainIdParent), ALICE.on(chainIdA), 999);
    }

    function runB() public view {
        // Check if the block chain id has been updated
        require(block.chainid == chainIdB);
    }

    function checkChainIds() public view {
        require(block.chainid == chainIdParent);
        require(on(chainIdA).getChainId() == chainIdA);
        require(block.chainid == chainIdParent);
        require(on(chainIdB).getChainId() == chainIdB);
        require(block.chainid == chainIdParent);
    }

    function getETHBalance(address addr) public view returns (uint) {
       return addr.balance;
    }

    function transferAndCheck(EVM.ChainAddr memory from, EVM.ChainAddr memory to, uint amount) internal {
        vm.startBroadcast(from.addr);

        uint from_balance_before = token_on(from.chain_id).balanceOf(from.addr);
        uint to_balance_before = token_on(to.chain_id).balanceOf(to.addr);

        token_on(from.chain_id).xTransfer(to.chain_id, to.addr, amount);

        uint from_balance_after = token_on(from.chain_id).balanceOf(from.addr);
        uint to_balance_after = token_on(to.chain_id).balanceOf(to.addr);

        require(from_balance_before == from_balance_after + amount);
        require(to_balance_before == to_balance_after - amount);

        vm.stopBroadcast();
    }

    function transferETHAndCheck(EVM.ChainAddr memory from, EVM.ChainAddr memory to, uint amount) internal {
        on(from.chain_id)._transferETHAndCheck(from, to, amount);
    }

    function _transferETHAndCheck(EVM.ChainAddr memory from, EVM.ChainAddr memory to, uint amount) external {
        xERC20 token = on(chainIdParent).token();

        vm.startBroadcast(from.addr);

        uint to_balance_before = on(to.chain_id).balance(to.addr);
        uint from_balance_before = on(from.chain_id).balance(from.addr);

        token.sendETH{value: amount}(to.chain_id, payable(to.addr));

        uint from_balance_after = on(from.chain_id).balance(from.addr);
        uint to_balance_after = on(to.chain_id).balance(to.addr);

        require(from_balance_before == from_balance_after + amount);
        require(to_balance_before == to_balance_after - amount);

        vm.stopBroadcast();
    }

    function token_on(uint chain_id) internal view returns (xERC20) {
        xERC20 token = on(chainIdParent).token();
        return xERC20(address(token).onChain(chain_id));
    }

    function balance(address addr) public view returns (uint) {
       return addr.balance;
    }

    function getChainId() public view returns (uint) {
       return block.chainid;
    }
}