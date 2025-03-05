# Uniswap Deployment Guide

This guide explains how to deploy and run Uniswap locally or from a local repository using the following components:

- **Smart Contracts**
- **SDK** (with chain support)
- **Interface/UI**

---

## ⚠️ Important Note

The deployment addresses (`FACTORY_ADDRESS`, `WETH`) below are valid **only if the first transactions made with the specified private key (`53321db7c1e331d93a11a41d16f004d7ff63972ec8ec7c25db329728ceeb1710`)** are the Uniswap contract deployments.  
- **Do not use this private key for any other transactions before deploying the Uniswap contracts.**  
- Otherwise, you must update the **Interface** and **SDK** repositories with the new deployment addresses.
- URLs below can change depending on if the nodes locally available or external ! (e.g.: localhost vs. l2a.rpc.gwyneth.xyz, etc.)

---

## 1. Uniswap Smart Contracts

1. Clone the repository:  
   ```bash
   git clone https://github.com/taikoxyz/uniswap-v2
2. Init submodules and build the bindings
   ```bash
   git submodule init
   forge build
3. Deploy the contracts on L1 (and L2A and L2B too for sync comp).
   ```bash
   $ forge script script/UniswapDeployer.s.sol --rpc-url http://localhost:32002(5 or 6) --broadcast --legacy
   $ forge script script/DeployTokens.s.sol --rpc-url http://localhost:32002(5 or 6) --broadcast --legacy
4. On L2s, we need UniswapPortal contracts too.
   $ forge script script/DeployPortal.s.sol --rpc-url http://localhost:32005 (6) --broadcast --legacy
## 2. Uniswap SDK

1. Clone the repository and switch to the `gwyneth_uniswapV2` branch:
   ```bash
   git clone https://github.com/adaki2004/v2-sdk && cd v2-sdk
   git checkout gwyneth_uniswapV2
2. Build the SDK:
   ```bash
   yarn && yarn build
> **_NOTE:_** Ensure that the contracts are deployed first before interacting with this repository using the specified private key.

## 3. Uniswap Interface/UI
> **_NOTE:_** Ensure that the SDK repository is in the same root directory as one, as it is referenced in `package.json` like this:
`"@uniswap/sdk": "file:../v2-sdk"`.
> **_NOTE:_** Step `nr. 2` and `nr. 3` can be shot up containerized too, from the interface repository, with `docker run -d -p 3000:3000 uniswap_ui` too.

1. Clone the repository:  
   ```bash
   git clone https://github.com/adaki2004/interface

2. Switch to the `gwyneth_uniswapV2` branch:
   ```bash
   git checkout gwyneth_uniswapV2
3. Deploy the contracts
   ```bash
   yarn
   export NODE_OPTIONS=--openssl-legacy-provider
   yarn start
## Additional Notes
Ensure that the repositories are properly structured in your working directory for dependency resolution.
If deployment addresses change, you will need to update the Interface and SDK configurations.

## One example E2E testing, you should be doing the following:

1. On `gwynethification` branch in the smart contract repository (https://github.com/taikoxyz/uniswap-v2), deploy the contracts: (before, do a git submodule init and update!)
   ```bash
   ./script/deployContracts.sh
2. On Uniswap Interface/UI repository, change to `xTransfer_UI` branch and shoot up the UI as described above.
Add liquidity (manually): a pool with 1M SLOTH + 200K Taiko tokens (amount not important, but tokens should be) - both on L1 and L2A.
3. Initiate a cross-swap in the smart contract repository with the command:
   ```bash
   forge script script/CrossSwap.s.sol --rpc-url http://localhost:32005 --broadcast --legacy -vvv --gas-estimate-multiplier 200
