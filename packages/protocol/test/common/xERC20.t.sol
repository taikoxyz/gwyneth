// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import "../TaikoTest.sol";
import "../../contracts/examples/xERC20.sol";
import "../../contracts/examples/EVM.sol";

contract TestXERC20 is TaikoTest {
    uint constant chainIdParent = 31337;
    uint constant chainIdA = 31338;
    uint constant chainIdB = 31339;

    using EVM for address;
    using EVM for address payable;

    xERC20 public token;

    function ChainAddress(uint256 chainId, TestXERC20 contractAddr) internal view returns (TestXERC20) {
        return TestXERC20(address(contractAddr).onChain(chainId));
    }

    function on(uint256 chainId) internal view returns (TestXERC20) {
        return ChainAddress(chainId, this);
    }

    function setUp() public {
        // Check if chain id is as expected
        assertEq(block.chainid, chainIdParent);

        // Make Alice the msg.sender so she gets some initial tokens
        vm.startPrank(Alice);

        token = new xERC20(100_000);
        // Alice should have received 100.000 tokens
        assertEq(token.balanceOf(Alice), 100_000);
    }

    function test_xerc20() external {
        token.transfer(Bob, 10_000);
        assertEq(token.balanceOf(Alice), 90_000);
        assertEq(token.balanceOf(Bob), 10_000);

        // Do a transfer on L1
        transferAndCheck(Alice.on(chainIdParent), Bob.on(chainIdParent), 10_000);

        // Transfer tokens to Carol on chainIdA
        transferAndCheck(Alice.on(chainIdParent), Carol.on(chainIdA), 5_000);

        // Check balance remains the same on chainIdParent
        assertEq(token.balanceOf(Carol), 0);

        // Transfer tokens from A to B
        transferAndCheck(Carol.on(chainIdA), Bob.on(chainIdB), 3_000);
    }

    function test_eth() external {
        vm.deal(Alice, 10_000 ether);

        assertEq(Alice.balance, 10_000 ether);

        transferETHAndCheck(Alice.on(chainIdParent), Bob.on(chainIdParent), 12 ether);
        transferETHAndCheck(Alice.on(chainIdParent), Carol.on(chainIdA), 23 ether);
        transferETHAndCheck(Bob.on(chainIdParent), David.on(chainIdA), 3 ether);

        transferETHAndCheck(Carol.on(chainIdA), Bob.on(chainIdA), 4 ether);
        transferETHAndCheck(Bob.on(chainIdA), Alice.on(chainIdB), 2 ether);

        transferETHAndCheck(Carol.on(chainIdA), Alice.on(chainIdParent), 1.5 ether);
        transferETHAndCheck(Alice.on(chainIdB), Bob.on(chainIdParent), 2 ether);
    }

    function transferAndCheck(EVM.ChainAddr memory from, EVM.ChainAddr memory to, uint amount) internal {
        vm.startPrank(from.addr);

        uint from_balance_before = token_on(from.chain_id).balanceOf(from.addr);
        uint to_balance_before = token_on(to.chain_id).balanceOf(to.addr);

        token_on(from.chain_id).xTransfer(to.chain_id, to.addr, amount);

        uint from_balance_after = token_on(from.chain_id).balanceOf(from.addr);
        uint to_balance_after = token_on(to.chain_id).balanceOf(to.addr);

        assertEq(from_balance_before, from_balance_after + amount);
        assertEq(to_balance_before, to_balance_after - amount);
    }

    function transferETHAndCheck(EVM.ChainAddr memory from, EVM.ChainAddr memory to, uint amount) internal {
        on(from.chain_id)._transferETHAndCheck(from, to, amount);
    }

    function _transferETHAndCheck(EVM.ChainAddr memory from, EVM.ChainAddr memory to, uint amount) external {
        xERC20 token = on(chainIdParent).token();

        vm.startPrank(from.addr);

        uint to_balance_before = on(to.chain_id).balance(to.addr);
        uint from_balance_before = on(from.chain_id).balance(from.addr);

        token.sendETH{value: amount}(to.chain_id, payable(to.addr));

        uint from_balance_after = on(from.chain_id).balance(from.addr);
        uint to_balance_after = on(to.chain_id).balance(to.addr);

        assertEq(from_balance_before, from_balance_after + amount);
        assertEq(to_balance_before, to_balance_after - amount);
    }

    function token_on(uint chain_id) internal returns (xERC20) {
        xERC20 token = on(chainIdParent).token();
        return xERC20(address(token).onChain(chain_id));
    }

    function balance(address addr) public returns (uint) {
       return addr.balance;
    }
}
