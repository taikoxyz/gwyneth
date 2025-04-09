//! A basic Ethereum payload builder implementation.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![allow(clippy::useless_let_if_seq)]

use std::sync::Arc;

use alloy_consensus::{Header, EMPTY_OMMER_ROOT_HASH};
use alloy_eips::{eip7685::Requests, merge::BEACON_NONCE};
use alloy_primitives::hex;
use alloy_primitives::B256;
use reth_basic_payload_builder::{
    commit_withdrawals, is_better_payload, BuildArguments, BuildOutcome, PayloadConfig,
    WithdrawalsOutcome,
};
use reth_chain_state::ExecutedBlock;
use reth_chainspec::ChainSpec;
use reth_chainspec::EthereumHardforks;
use reth_evm::{system_calls::SystemCaller, ConfigureEvm};
use reth_evm_ethereum::eip6110::parse_deposits_from_receipts;
use reth_execution_types::ExecutionOutcome;
use reth_node_api::PayloadBuilderError;
use reth_payload_builder::EthBuiltPayload;
use reth_primitives::Receipts;
use reth_primitives::{proofs, Block, BlockBody, Receipt};
use reth_provider::{ChainSpecProvider, StateProviderFactory};
use reth_revm::{
    cached::to_sync_cached_reads,
    database::{StateProviderDatabase, SyncStateProviderDatabase},
};
use reth_transaction_pool::{
    EthPooledTransaction, PoolTransaction,
    TransactionPool,
};
use reth_trie::HashedPostState;

use reth_errors::RethError;
use revm::{
    db::{states::bundle_state::BundleRetention, State},
    primitives::{
        calc_excess_blob_gas, BlockEnv, CfgEnvWithHandlerCfg, EVMError, EnvWithHandlerCfg,
        InvalidTransaction, ResultAndState, TxEnv, U256,
    },
    DatabaseCommit,
};
use tracing::{debug, trace, warn};

use crate::GwynethPayloadBuilderAttributes;

/// Constructs an Ethereum transaction payload using the best transactions from the pool.
///
/// Given build arguments including an Ethereum client, transaction pool,
/// and configuration, this function creates a transaction payload. Returns
/// a result indicating success with the payload or an error in case of failure.
#[inline]
pub fn default_gwyneth_payload<EvmConfig, Pool, Client>(
    evm_config: EvmConfig,
    args: BuildArguments<Pool, Client, GwynethPayloadBuilderAttributes, EthBuiltPayload>,
    initialized_cfg: CfgEnvWithHandlerCfg,
    initialized_block_env: BlockEnv,
) -> Result<BuildOutcome<EthBuiltPayload>, PayloadBuilderError>
where
    EvmConfig: ConfigureEvm<Header = Header>,
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec = ChainSpec>,
    // SP: StateProviderFactory + ChainSpecProvider,
    Pool: TransactionPool,
{
    let BuildArguments { client, pool, cached_reads, config, cancel, best_payload } = args;
    let PayloadConfig { parent_header, extra_data, attributes } = config;

    let chain_spec = client.chain_spec();
    let state_provider = client.state_by_block_hash(parent_header.hash())?;
    let state = StateProviderDatabase::new(Arc::new(state_provider));

    let mut sync_cached_reads = to_sync_cached_reads(cached_reads, chain_spec.chain.id());
    let mut sync_state = SyncStateProviderDatabase::new(Some(chain_spec.chain().id()), state);

    let sync_providers = attributes.sync_provider.expect("sync provider is required");
    sync_providers.into_iter().for_each(|(id, provider)| {
        sync_state.add_db(id, StateProviderDatabase::new(provider));
    });

    let mut sync_db = State::builder()
        .with_database_ref(sync_cached_reads.as_db(sync_state))
        .with_bundle_update()
        .build();


    debug!(target: "payload_builder", id=%attributes.inner.id, parent_hash = ?parent_header.hash(), parent_number = parent_header.number, "building new payload");
    let mut cumulative_gas_used = 0;
    let sum_blob_gas_used = 0;
    let block_gas_limit: u64 =
        initialized_block_env.gas_limit.try_into().unwrap_or(chain_spec.max_gas_limit);
    let base_fee = initialized_block_env.basefee.to::<u64>();

    // let mut executed_txs = Vec::new();
    // let mut executed_senders = Vec::new();

    // let mut best_txs = pool.best_transactions_with_attributes(BestTransactionsAttributes::new(
    //     base_fee,
    //     initialized_block_env.get_blob_gasprice().map(|gasprice| gasprice as u64),
    // ));

    let total_fees = U256::from(1u64);

    let block_number = initialized_block_env.number.to::<u64>();

    // let mut system_caller: SystemCaller<EvmConfig, ChainSpec> = SystemCaller::new(evm_config.clone(), chain_spec.clone());

    // // apply eip-4788 pre block contract call
    // system_caller
    //     .pre_block_beacon_root_contract_call(
    //         &mut sync_db,
    //         &initialized_cfg,
    //         &initialized_block_env,
    //         attributes.parent_beacon_block_root,
    //     )
    //     .map_err(|err| {
    //         warn!(target: "payload_builder",
    //             parent_hash=%parent_header.hash(),
    //             %err,
    //             "failed to apply beacon root contract call for payload"
    //         );
    //         PayloadBuilderError::Internal(err.into())
    //     })?;

    // // apply eip-2935 blockhashes update
    // system_caller.pre_block_blockhashes_contract_call(
    //     &mut sync_db,
    //     &initialized_cfg,
    //     &initialized_block_env,
    //     parent_header.hash(),
    // )
    // .map_err(|err| {
    //     warn!(target: "payload_builder", parent_hash=%parent_header.hash(), %err, "failed to update parent header blockhashes for payload");
    //     PayloadBuilderError::Internal(err.into())
    // })?;

    // let env = EnvWithHandlerCfg::new_with_cfg_env(
    //     initialized_cfg.clone(),
    //     initialized_block_env.clone(),
    //     TxEnv::default(),
    // );
    // let mut evm = evm_config.evm_with_env(&mut sync_db, env);

    // let mut receipts = Vec::new();
    // for tx in transactions {
    //     let pool_tx = EthPooledTransaction::new(
    //         tx.clone().1.try_into_ecrecovered().unwrap(),
    //         // TODO: used to limit the tx pool size doesn't matter here
    //         200,
    //     );
    //     if cumulative_gas_used + pool_tx.gas_limit() > block_gas_limit {
    //         // we can't fit this transaction into the block, so we need to mark it as invalid
    //         // which also removes all dependent transaction from the iterator before we can
    //         // continue
    //         // best_txs.mark_invalid(&pool_tx);
    //         continue;
    //     }

    //     // check if the job was cancelled, if so we can exit early
    //     if cancel.is_cancelled() {
    //         return Ok(BuildOutcome::Cancelled);
    //     }

    //     // convert tx to a signed transaction
    //     let tx = pool_tx.transaction().clone();

    //     // // There's only limited amount of blob space available per block, so we need to check if
    //     // // the EIP-4844 can still fit in the block
    //     // if let Some(blob_tx) = tx.transaction.as_eip4844() {
    //     //     let tx_blob_gas = blob_tx.blob_gas();
    //     //     if sum_blob_gas_used + tx_blob_gas > MAX_DATA_GAS_PER_BLOCK {
    //     //         // we can't fit this _blob_ transaction into the block, so we mark it as
    //     //         // invalid, which removes its dependent transactions from
    //     //         // the iterator. This is similar to the gas limit condition
    //     //         // for regular transactions above.
    //     //         trace!(target: "payload_builder", tx=?tx.hash, ?sum_blob_gas_used, ?tx_blob_gas, "skipping blob transaction because it would exceed the max data gas per block");
    //     //         best_txs.mark_invalid(&pool_tx);
    //     //         continue;
    //     //     }
    //     // }

    //     // Configure the environment for the tx.
    //     *evm.tx_mut() = evm_config.tx_env(tx.as_signed(), tx.signer());

    //     let ResultAndState { result, state } = match evm.transact() {
    //         Ok(res) => res,
    //         Err(err) => {
    //             match err {
    //                 EVMError::Transaction(err) => {
    //                     if matches!(err, InvalidTransaction::NonceTooLow { .. }) {
    //                         // if the nonce is too low, we can skip this transaction
    //                         trace!(target: "payload_builder", %err, ?tx, "skipping nonce too low transaction");
    //                     } else {
    //                         // if the transaction is invalid, we can skip it and all of its
    //                         // descendants
    //                         trace!(target: "payload_builder", %err, ?tx, "skipping invalid transaction and its descendants");
    //                         // best_txs.mark_invalid(&pool_tx);
    //                     }

    //                     continue;
    //                 }
    //                 err => {
    //                     // this is an error that we should treat as fatal for this attempt
    //                     return Err(PayloadBuilderError::EvmExecutionError(err));
    //                 }
    //             }
    //         }
    //     };

    //     // commit changes
    //     evm.db_mut().commit(state);

    //     // add to the total blob gas used if the transaction successfully executed
    //     // if let Some(blob_tx) = tx.transaction.as_eip4844() {
    //     //     let tx_blob_gas = blob_tx.blob_gas();
    //     //     sum_blob_gas_used += tx_blob_gas;

    //     //     // if we've reached the max data gas per block, we can skip blob txs entirely
    //     //     if sum_blob_gas_used == MAX_DATA_GAS_PER_BLOCK {
    //     //         best_txs.skip_blobs();
    //     //     }
    //     // }

    //     let gas_used = result.gas_used();

    //     // add gas used by the transaction to cumulative gas used, before creating the receipt
    //     cumulative_gas_used += gas_used;

    //     // Push transaction changeset and calculate header bloom filter for receipt.
    //     #[allow(clippy::needless_update)] // side-effect of optimism fields
    //     receipts.push(Some(Receipt {
    //         tx_type: tx.tx_type(),
    //         success: result.is_success(),
    //         cumulative_gas_used,
    //         logs: result.into_logs().into_iter().map(Into::into).collect(),
    //         ..Default::default()
    //     }));

    //     // update add to total fees
    //     let miner_fee = tx
    //         .effective_tip_per_gas(Some(base_fee))
    //         .expect("fee is always valid; execution succeeded");
    //     total_fees += U256::from(miner_fee) * U256::from(gas_used);

    //     // append sender and transaction to the respective lists
    //     executed_senders.push(tx.signer());
    //     executed_txs.push(tx.into_signed());
    // }

    // Release db
    // drop(evm);

    // check if we have a better block
    if !is_better_payload(best_payload.as_ref(), total_fees) {
        // can skip building the block
        return Ok(BuildOutcome::Aborted {
            fees: total_fees,
            cached_reads: sync_cached_reads.into(),
        });
    }

    // // calculate the requests and the requests root
    // let requests = if chain_spec.is_prague_active_at_timestamp(attributes.timestamp) {
    //     let deposit_requests = parse_deposits_from_receipts(&chain_spec, receipts.iter().flatten())
    //         .map_err(|err| PayloadBuilderError::Internal(RethError::Execution(err.into())))?;
    //     let withdrawal_requests = system_caller
    //         .post_block_withdrawal_requests_contract_call(
    //             &mut sync_db,
    //             &initialized_cfg,
    //             &initialized_block_env,
    //         )
    //         .map_err(|err| PayloadBuilderError::Internal(err.into()))?;
    //     let consolidation_requests = system_caller
    //         .post_block_consolidation_requests_contract_call(
    //             &mut sync_db,
    //             &initialized_cfg,
    //             &initialized_block_env,
    //         )
    //         .map_err(|err| PayloadBuilderError::Internal(err.into()))?;

    //     Some(Requests::new(vec![deposit_requests, withdrawal_requests, consolidation_requests]))
    // } else {
    //     None
    // };

    // let WithdrawalsOutcome { withdrawals_root, withdrawals } = commit_withdrawals(
    //     chain_spec.chain.id(),
    //     &mut sync_db,
    //     &chain_spec,
    //     attributes.timestamp,
    //     attributes.withdrawals,
    // )?;

    // merge all transitions into bundle state, this would apply the withdrawal balance changes
    // and 4788 contract call
    // sync_db.merge_transitions(BundleRetention::Reverts);

    // let requests_hash = requests.as_ref().map(|requests| requests.requests_hash());

    let execution_outcome = ExecutionOutcome::new(
        chain_spec.chain().id(),
        attributes.chain_da.state_diff.clone().unwrap().bundle.clone(),
        vec![attributes.chain_da.state_diff.clone().unwrap().receipts.iter().map(|r| Some(r.clone())).collect::<Vec<_>>()].into(),
        block_number,
        vec![Requests::default()],
    )
    .filter_current_chain();
    println!(
        "🥳 L2Builder filtering execution_outcome for {:?} at {} | {} receipts found", 
        chain_spec.chain().id(), block_number, execution_outcome.receipts().len()
    );

    let receipts_root =
        execution_outcome.receipts_root_slow(block_number).expect("Number is in range");
    let logs_bloom = execution_outcome.block_logs_bloom(block_number).expect("Number is in range");

    // calculate the state root
    let hashed_state = HashedPostState::from_bundle_state(&execution_outcome.current_state().state);
    let (state_root, trie_output) = {
        let chain_state =
            sync_db.database.0.inner.get_mut().db.get_db(chain_spec.chain.id()).unwrap();
        chain_state.state_root_with_updates(hashed_state.clone()).inspect_err(|err| {
            warn!(target: "payload_builder",
                parent_hash=%parent_header.hash(),
                %err,
                "failed to calculate state root for payload"
            );
        })?
    };


    // create the block header
    // let transactions_root = proofs::calculate_transaction_root(&executed_txs);

    // initialize empty blob sidecars at first. If cancun is active then this will
    // let mut blob_sidecars = Vec::new();
    // let mut excess_blob_gas: Option<u64> = None;
    // let mut blob_gas_used: Option<u64> = None;

    // only determine cancun fields when active
    // if chain_spec.is_cancun_active_at_timestamp(attributes.inner.timestamp) {
    //     // grab the blob sidecars from the executed txs
    //     blob_sidecars = pool
    //         .get_all_blobs_exact(
    //             executed_txs.iter().filter(|tx| tx.is_eip4844()).map(|tx| tx.hash).collect(),
    //         )
    //         .map_err(PayloadBuilderError::other)?;

    //     excess_blob_gas = if chain_spec.is_cancun_active_at_timestamp(parent_header.timestamp) {
    //         let parent_excess_blob_gas = parent_header.excess_blob_gas.unwrap_or_default();
    //         let parent_blob_gas_used = parent_header.blob_gas_used.unwrap_or_default();
    //         Some(calc_excess_blob_gas(parent_excess_blob_gas, parent_blob_gas_used))
    //     } else {
    //         // for the first post-fork block, both parent.blob_gas_used and
    //         // parent.excess_blob_gas are evaluated as 0
    //         Some(calc_excess_blob_gas(0, 0))
    //     };

    //     blob_gas_used = Some(sum_blob_gas_used);
    // }
    let state_diff = attributes.chain_da.state_diff.clone().unwrap();

    if state_diff.state_root != state_root {
        println!("State root mismatch! {} {}", state_diff.state_root, state_root);
    }


    let header = Header {
        parent_hash: parent_header.hash(),
        ommers_hash: EMPTY_OMMER_ROOT_HASH,
        beneficiary: initialized_block_env.coinbase.1,
        state_root: state_diff.state_root,
        transactions_root: state_diff.transactions_root,
        receipts_root,
        withdrawals_root: Some(B256::from(hex!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"))),
        logs_bloom,
        timestamp: attributes.inner.timestamp,
        mix_hash: attributes.inner.prev_randao,
        nonce: BEACON_NONCE.into(),
        base_fee_per_gas: Some(base_fee),
        number: parent_header.number + 1,
        gas_limit: block_gas_limit,
        difficulty: U256::ZERO,
        //gas_used: cumulative_gas_used,
        gas_used: state_diff.gas_used,
        extra_data: attributes.chain_da.extra_data.clone(),
        parent_beacon_block_root: attributes.inner.parent_beacon_block_root,
        blob_gas_used: Some(0),
        excess_blob_gas: Some(0),
        requests_hash: None,
    };
    println!("🥳 L2Builder default_gwyneth_payload {:?}", header.number);

    let executed_txs = attributes.transactions.iter().cloned().map(|tx| tx.1.try_into_ecrecovered().unwrap().into_signed()).collect::<Vec<_>>();


    // seal the block
    let block = Block {
        header,
        body: BlockBody { transactions: executed_txs, ommers: vec![], withdrawals: None },
    };
    let sealed_block = Arc::new(block.seal_slow());
    debug!(target: "payload_builder", ?sealed_block, "sealed built block");

    // create the executed block data
    let executed = ExecutedBlock {
        block: sealed_block.clone(),
        senders: Arc::new(Vec::new()),
        execution_output: Arc::new(execution_outcome),
        hashed_state: Arc::new(hashed_state),
        trie: Arc::new(trie_output),
    };

    let mut payload =
        EthBuiltPayload::new(attributes.inner.id, sealed_block, total_fees, Some(executed), Some(Requests::default()));

    // extend the payload with the blob sidecars from the executed txs
    // payload.extend_sidecars(blob_sidecars.into_iter().map(Arc::unwrap_or_clone));

    Ok(BuildOutcome::Better { payload, cached_reads: sync_cached_reads.into() })
}
