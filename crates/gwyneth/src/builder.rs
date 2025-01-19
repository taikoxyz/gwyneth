//! A basic Ethereum payload builder implementation.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![allow(clippy::useless_let_if_seq)]

use std::collections::HashMap;

use reth_basic_payload_builder::{
    commit_withdrawals, is_better_payload, BuildArguments, BuildOutcome, PayloadConfig,
    WithdrawalsOutcome,
};
use reth_errors::RethError;
use reth_evm::{
    execute::BlockExecutionOutput,
    ConfigureEvm,
};
use reth_evm_ethereum::eip6110::parse_deposits_from_receipts;
use reth_execution_types::ExecutionOutcome;
use reth_provider::{state_diff_to_block_execution_output, ChainSpecProvider, StateProvider, StateProviderFactory};
use reth_revm::database::{StateProviderDatabase, SyncStateProviderDatabase};
use reth_trie::HashedPostState;
use revm::{
    db::{states::bundle_state::BundleRetention, BundleAccount, BundleState, State},
    primitives::{calc_excess_blob_gas, BlockEnv, EVMError, EnvWithHandlerCfg, CfgEnvWithHandlerCfg, ResultAndState, InvalidTransaction, TxEnv},
    DatabaseCommit, SyncDatabase,
};
use reth_chainspec::ChainSpec;
use alloy_primitives::{U256, B256, BlockNumber, ChainId};
use alloy_eips::{eip4844::MAX_DATA_GAS_PER_BLOCK, eip7685::Requests, merge::BEACON_NONCE};
use reth_primitives::{
    proofs::{self},
    Block, BlockBody, EthereumHardforks, Receipt,
};

use tracing::{debug, trace, warn};

use reth_revm::primitives::ChainAddress;
use reth_revm::db::AccountStatus;
use reth_revm::db::states::StorageSlot;
use reth_revm::cached::to_sync_cached_reads;
use alloy_eips::eip4895::Withdrawals;
use alloy_consensus::Receipts;
use reth_node_api::PayloadBuilderAttributes;

use reth_primitives::GwynethDA;

use reth_execution_types::execution_outcome_to_state_diff;

use alloy_primitives::hex;

use reth_ethereum_engine_primitives::{
    EthBuiltPayload, EthPayloadAttributes, EthPayloadBuilderAttributes, EthereumEngineValidator,
};
use reth_payload_builder::PayloadBuilderError;
use alloy_consensus::{Header, EMPTY_OMMER_ROOT_HASH};

use crate::GwynethPayloadBuilderAttributes;

use reth_transaction_pool::{
    noop::NoopTransactionPool, BestTransactions, BestTransactionsAttributes, TransactionPool,
    ValidPoolTransaction,
};
use reth_revm::cached::CachedReads;

use std::sync::Arc;
use crate::EthEvmConfig;
use reth_evm::NextBlockEnvAttributes;
use crate::PayloadBuilder;

use reth_chain_state::ExecutedBlock;

type BestTransactionsIter<Pool> = Box<
    dyn BestTransactions<Item = Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>,
>;

/// Ethereum payload builder
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GwynethPayloadBuilder<EvmConfig = EthEvmConfig> {
    /// The type responsible for creating the evm.
    evm_config: EvmConfig,
}

impl<EvmConfig> GwynethPayloadBuilder<EvmConfig> {
    /// `GwynethPayloadBuilder` constructor.
    pub const fn new(evm_config: EvmConfig) -> Self {
        Self { evm_config }
    }
}

impl<EvmConfig> GwynethPayloadBuilder<EvmConfig>
where
    EvmConfig: ConfigureEvm<Header = Header>,
{
    /// Returns the configured [`CfgEnvWithHandlerCfg`] and [`BlockEnv`] for the targeted payload
    /// (that has the `parent` as its parent).
    fn cfg_and_block_env(
        &self,
        config: &PayloadConfig<GwynethPayloadBuilderAttributes>,
        parent: &Header,
    ) -> Result<(CfgEnvWithHandlerCfg, BlockEnv), EvmConfig::Error> {
        let next_attributes = NextBlockEnvAttributes {
            timestamp: config.attributes.inner.timestamp(),
            suggested_fee_recipient: config.attributes.suggested_fee_recipient(),
            prev_randao: config.attributes.prev_randao(),
        };
        self.evm_config.next_cfg_and_block_env(parent, next_attributes)
    }
}

// Default implementation of [PayloadBuilder] for unit type
impl<EvmConfig, Pool, Client> PayloadBuilder<Pool, Client> for GwynethPayloadBuilder<EvmConfig>
where
    EvmConfig: ConfigureEvm<Header = Header>,
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec = ChainSpec>,
    Pool: TransactionPool,
{
    type Attributes = GwynethPayloadBuilderAttributes;
    type BuiltPayload = EthBuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<Pool, Client, GwynethPayloadBuilderAttributes, EthBuiltPayload>,
    ) -> Result<BuildOutcome<EthBuiltPayload>, PayloadBuilderError> {
        let (cfg_env, block_env) = self
            .cfg_and_block_env(&args.config, &args.config.parent_header)
            .map_err(PayloadBuilderError::other)?;

        let pool = args.pool.clone();
        default_gwyneth_payload(self.evm_config.clone(), args, cfg_env, block_env, |attributes| {
            pool.best_transactions_with_attributes(attributes)
        })
    }

    fn build_empty_payload(
        &self,
        client: &Client,
        config: PayloadConfig<GwynethPayloadBuilderAttributes>,
    ) -> Result<EthBuiltPayload, PayloadBuilderError> {
        let args = BuildArguments::new(
            client,
            // we use defaults here because for the empty payload we don't need to execute anything
            NoopTransactionPool::default(),
            Default::default(),
            config,
            Default::default(),
            None,
        );

        let (cfg_env, block_env) = self
            .cfg_and_block_env(&args.config, &args.config.parent_header)
            .map_err(PayloadBuilderError::other)?;

        let pool = args.pool.clone();

        default_gwyneth_payload(self.evm_config.clone(), args, cfg_env, block_env, |attributes| {
            pool.best_transactions_with_attributes(attributes)
        })?
        .into_payload()
        .ok_or_else(|| PayloadBuilderError::MissingPayload)
    }
}


/// Constructs an Ethereum transaction payload using the best transactions from the pool.
///
/// Given build arguments including an Ethereum client, transaction pool,
/// and configuration, this function creates a transaction payload. Returns
/// a result indicating success with the payload or an error in case of failure.
// #[inline]
// pub fn default_gwyneth_payload_builder<EvmConfig, Pool, Client, SyncProvider>(
//     evm_config: EvmConfig,
//     args: BuildArguments<
//         Pool,
//         Client,
//         GwynethPayloadBuilderAttributes<SyncProvider>,
//         EthBuiltPayload,
//     >,
// ) -> Result<BuildOutcome<EthBuiltPayload>, PayloadBuilderError>
// where
//     EvmConfig: ConfigureEvm,
//     Client: StateProviderFactory,
//     Pool: TransactionPool,
//     SyncProvider: StateProvider,
// {
pub fn default_gwyneth_payload<EvmConfig, Pool, Client, F>(
    evm_config: EvmConfig,
    args: BuildArguments<Pool, Client, GwynethPayloadBuilderAttributes, EthBuiltPayload>,
    initialized_cfg: CfgEnvWithHandlerCfg,
    initialized_block_env: BlockEnv,
    best_txs: F,
) -> Result<BuildOutcome<EthBuiltPayload>, PayloadBuilderError>
where
    EvmConfig: ConfigureEvm<Header = Header>,
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec = ChainSpec>,
    Pool: TransactionPool,
    F: FnOnce(BestTransactionsAttributes) -> BestTransactionsIter<Pool>,
{
    let BuildArguments { client, pool, mut cached_reads, config, cancel, best_payload } = args;
    let PayloadConfig {
        parent_header,
        extra_data,
        attributes,
    } = config;

    let chain_spec = client.chain_spec();

    println!("[{}] Build", chain_spec.chain().id());

    let state_provider = client.state_by_block_hash(parent_header.hash())?;
    let state = StateProviderDatabase::new(state_provider);
    let mut sync_state = SyncStateProviderDatabase::new(Some(chain_spec.chain().id()), state);

    // Add all external state dependencies
    // for (&chain_id, provider) in attributes.providers.iter() {
    //     //println!("Adding db for chain_id: {}", chain_id);
    //     let boxed: Box<dyn StateProvider> = Box::new(provider);
    //     let state_provider = StateProviderDatabase::new(boxed);
    //     sync_state.add_db(chain_id, state_provider);
    // }

    let mut sync_cached_reads = to_sync_cached_reads(cached_reads, chain_spec.chain.id());
    let mut sync_db = State::builder()
        .with_database_ref(sync_cached_reads.as_db(sync_state))
        .with_bundle_update()
        .build();

    debug!(target: "payload_builder", id=%attributes.inner.id, parent_hash = ?parent_header.hash(), parent_number = parent_header.number, "building new payload");
    let mut cumulative_gas_used = 0;
    let mut sum_blob_gas_used = 0;
    let block_gas_limit: u64 = chain_spec.max_gas_limit;
    let base_fee = initialized_block_env.basefee.to::<u64>();

    //let mut executed_txs: Vec<TransactionSigned> = Vec::new();

    // let mut best_txs = pool.best_transactions_with_attributes(BestTransactionsAttributes::new(
    //     base_fee,
    //     initialized_block_env.get_blob_gasprice().map(|gasprice| gasprice as u64),
    // ));

    let mut total_fees = U256::ZERO;

    let block_number = initialized_block_env.number.to::<u64>();

    // apply eip-4788 pre block contract call
    // pre_block_beacon_root_contract_call(
    //     &mut sync_db,
    //     &evm_config,
    //     &chain_spec,
    //     &initialized_cfg,
    //     &initialized_block_env,
    //     attributes.inner.parent_beacon_block_root,
    // )
    // .map_err(|err| {
    //     warn!(target: "payload_builder",
    //         parent_hash=%parent_block.hash(),
    //         %err,
    //         "failed to apply beacon root contract call for empty payload"
    //     );
    //     PayloadBuilderError::Internal(err.into())
    // })?;

    // // apply eip-2935 blockhashes update
    // pre_block_blockhashes_contract_call(
    //     &mut sync_db,
    //     &evm_config,
    //     &chain_spec,
    //     &initialized_cfg,
    //     &initialized_block_env,
    //     parent_block.hash(),
    // )
    // .map_err(|err| {
    //     warn!(target: "payload_builder", parent_hash=%parent_block.hash(), %err, "failed to update blockhashes for empty payload");
    //     PayloadBuilderError::Internal(err.into())
    // })?;

    // let mut receipts = Vec::new();
    // for tx in &attributes.transactions {
    //     // // ensure we still have capacity for this transaction
    //     // if cumulative_gas_used + tx.gas_limit() > block_gas_limit {
    //     //     // we can't fit this transaction into the block, so we need to mark it as invalid
    //     //     // which also removes all dependent transaction from the iterator before we can
    //     //     // continue
    //     //     best_txs.mark_invalid(&tx);
    //     //     continue
    //     // }

    //     // Check if the job was cancelled, if so we can exit early.
    //     if cancel.is_cancelled() {
    //         return Ok(BuildOutcome::Cancelled)
    //     }

    //     let tx = tx.clone().1.try_into_ecrecovered().unwrap();

    //     // // There's only limited amount of blob space available per block, so we need to check if
    //     // // the EIP-4844 can still fit in the block
    //     // if let Some(blob_tx) = tx.transaction.as_eip4844() {
    //     //     let tx_blob_gas = blob_tx.blob_gas();
    //     //     if sum_blob_gas_used + tx_blob_gas > MAX_DATA_GAS_PER_BLOCK {
    //     //         // we can't fit this _blob_ transaction into the block, so we mark it as
    //     //         // invalid, which removes its dependent transactions from
    //     //         // the iterator. This is similar to the gas limit condition
    //     //         // for regular transactions above.
    //     //         trace!(target: "payload_builder", tx=?tx.hash, ?sum_blob_gas_used, ?tx_blob_gas,
    //     // "skipping blob transaction because it would exceed the max data gas per block");
    //     //         best_txs.mark_invalid(&tx);
    //     //         continue
    //     //     }
    //     // }

    //     let env = EnvWithHandlerCfg::new_with_cfg_env(
    //         initialized_cfg.clone(),
    //         initialized_block_env.clone(),
    //         evm_config.tx_env(&tx),
    //     );

    //     // Configure the environment for the block.
    //     let mut evm = evm_config.evm_with_env(&mut sync_db, env);

    //     let ResultAndState { result, state } = match evm.transact() {
    //         Ok(res) => res,
    //         Err(err) => {
    //             match err {
    //                 EVMError::Transaction(err) => {
    //                     println!("build tx error: {:?}", err);
    //                     trace!(target: "payload_builder", %err, ?tx, "Error in sequencer transaction, skipping.");
    //                     continue
    //                 }
    //                 err => {
    //                     println!("build tx error fatal: {:?}", err);
    //                     // this is an error that we should treat as fatal for this attempt
    //                     //return Err(PayloadBuilderError::EvmExecutionError(err))
    //                     continue
    //                 }
    //             }
    //         }
    //     };
    //     // drop evm so db is released.
    //     drop(evm);
    //     // commit changes
    //     sync_db.commit(state);

    //     // add to the total blob gas used if the transaction successfully executed
    //     if let Some(blob_tx) = tx.transaction.as_eip4844() {
    //         let tx_blob_gas = blob_tx.blob_gas();
    //         sum_blob_gas_used += tx_blob_gas;

    //         // if we've reached the max data gas per block, we can skip blob txs entirely
    //         if sum_blob_gas_used == MAX_DATA_GAS_PER_BLOCK {
    //             best_txs.skip_blobs();
    //         }
    //     }

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

    //     // append transaction to the list of executed transactions
    //     executed_txs.push(tx.into_signed());
    // }

    //println!("{} - Build: {:?}", chain_spec.chain().id(), executed_txs);

    total_fees = U256::from(1u64);

    // check if we have a better block
    if !is_better_payload(best_payload.as_ref(), total_fees) {
        // can skip building the block
        return Ok(BuildOutcome::Aborted {
           fees: total_fees,
           cached_reads: sync_cached_reads.into(),
        })
    }

    // merge all transitions into bundle state, this would apply the withdrawal balance changes
    // and 4788 contract call
    //sync_db.merge_transitions(BundleRetention::PlainState);

    // let execution_outcome = ExecutionOutcome::new(
    //     Some(chain_spec.chain().id()),
    //     sync_db.take_bundle(),
    //     vec![receipts].into(),
    //     block_number,
    //     vec![Requests::default()],
    // )
    // .filter_current_chain();

    let execution_outcome = ExecutionOutcome::new(
        chain_spec.chain().id(),
        attributes.chain_da.state_diff.clone().unwrap().bundle.clone(),
        vec![attributes.chain_da.state_diff.clone().unwrap().receipts.iter().map(|r| Some(r.clone())).collect::<Vec<_>>()].into(),
        block_number,
        vec![Requests::default()],
    )
    .filter_current_chain();

    let receipts_root =
        execution_outcome.receipts_root_slow(block_number).expect("Number is in range");
    let logs_bloom = execution_outcome.block_logs_bloom(block_number).expect("Number is in range");

    // calculate the state root
    let hashed_state = HashedPostState::from_bundle_state(&execution_outcome.current_state().state);
    let (state_root, trie_output) = {
        let state_provider = sync_db.database.0.inner.borrow_mut();
        state_provider.db.get_db(chain_spec.chain().id()).unwrap().state_root_with_updates(hashed_state.clone()).inspect_err(|err| {
            warn!(target: "payload_builder",
                parent_hash=%parent_header.hash(),
                %err,
                "failed to calculate state root for payload"
            );
        })?
    };

    // let state_root = {
    //     let state_provider = sync_db.database.0.inner.borrow_mut();
    //     state_provider.db.get_db(chain_spec.chain().id()).unwrap().state_root(
    //         HashedPostState::from_bundle_state(&execution_outcome.current_state().state),
    //     )?
    // };

    //let state_diff = execution_outcome_to_state_diff(&execution_outcome, state_root, cumulative_gas_used);

    let state_diff = attributes.chain_da.state_diff.clone().unwrap();

    assert!(state_diff.bundle.reverts.len() <= 1, "reverts need to be per block");

    // let execution_output_state_diff = state_diff_to_block_execution_output(chain_spec.chain().id(), &attributes.chain_da.state_diff.unwrap());
    // let state_root_2 = {
    //     let state_provider = sync_db.database.0.inner.borrow_mut();
    //     state_provider.db.get_db(chain_spec.chain().id()).unwrap().state_root(
    //         HashedPostState::from_bundle_state(&execution_outcome.current_state().state),
    //     )?
    // };

    // let state_root_2 = ParallelStateRoot::new(consistent_view, hashed_state)
    //     .incremental_root_with_updates()
    //     .map(|(root, updates)| (root, Some(updates)))
    //     .map_err(ProviderError::from)?;

    let executed_txs = attributes.transactions.iter().cloned().map(|tx| tx.try_into_ecrecovered().unwrap().into_signed()).collect::<Vec<_>>();
    let executed_senders = attributes.transactions.iter().cloned().map(|tx| tx.try_into_ecrecovered().unwrap().signer()).collect::<Vec<_>>();


    // create the block header
    //let transactions_root = proofs::calculate_transaction_root(&executed_txs);

    // Put the state diff into the extra bytes of the block
    //let extra_data = Bytes::from(serde_json::to_string(&state_diff).unwrap().into_bytes());
    //let extra_data = Bytes::from(bincode::serialize(&state_diff).unwrap());

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

    //println!("header: {:?}", header);

    //println!("[{}-{}] receipts: {:?}", chain_spec.chain().id(), header.number, state_diff.receipts);

    //println!("extra_data: {}", attributes.chain_da.extra_data);

    if state_diff.state_root != state_root {
        println!("State root mismatch! {} {}", state_diff.state_root, state_root);
    }

    //println!("reverts: {:?}", state_diff.bundle.reverts);

    // seal the block
    let block = Block {
        header,
        body: BlockBody {
            transactions: executed_txs,
            ommers: vec![],
            withdrawals: Some(Withdrawals::default()),
            //requests: Some(Requests::default()),
        },
    };

    let sealed_block = Arc::new(block.seal_slow());
    //sealed_block.state_diff = Some(execution_outcome_to_state_diff(&execution_outcome));

    //println!("block hash: {:?}", sealed_block.hash());

    //println!("[{}] execution outcome: {:?}", chain_spec.chain.id(), execution_outcome);

    //assert_eq!(state_diff, attributes.chain_da.state_diff.unwrap());

    debug!(target: "payload_builder", ?sealed_block, "sealed built block");

    // create the executed block data
    let executed = ExecutedBlock {
        block: sealed_block.clone(),
        senders: Arc::new(executed_senders),
        execution_output: Arc::new(execution_outcome),
        hashed_state: Arc::new(hashed_state),
        trie: Arc::new(trie_output),
    };

    let requests = Requests::default();
    let mut payload =
        EthBuiltPayload::new(attributes.inner.id, sealed_block, total_fees, Some(executed), Some(requests));

    //let payload = EthBuiltPayload::new(attributes.inner.id, sealed_block, total_fees);

    Ok(BuildOutcome::Better { payload, cached_reads: /*sync_cached_reads.into())*/ CachedReads::default() })
}

// pub fn build_execution_outcome<DB: SyncDatabase>(
//     sync_db: &mut State<DB>,
//     receipts: Receipts,
//     first_block: BlockNumber,
//     requests: Vec<Requests>,
// ) -> HashMap<ChainId, ExecutionOutcome> {
//     let bundle_states = sync_db.take_bundle();

//     todo!()
// }
