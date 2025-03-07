use std::{
    marker::PhantomData,
    sync::{Arc, RwLock},
};

use alloy_eips::BlockNumHash;
use alloy_primitives::{address, map::HashMap, Address, B256, U256};
use alloy_rlp::Decodable;
use alloy_rpc_types::engine::PayloadStatusEnum;
use alloy_sol_types::{sol, SolEventInterface};
use futures::{StreamExt, TryStreamExt};
// use reth::{network::NetworkHandle, rpc::eth::EthApi};
use reth_network::NetworkHandle;
use reth_primitives::{SealedBlock, SealedBlockWithSenders, SealedHeader, TransactionSigned};
use reth_rpc::EthApi;

use crate::RollupContract::{BlockProposed, RollupContractEvents};
use crate::{
    engine_api::EngineApiContext, GwynethEngineTypes, GwynethEngineValidatorBuilder, GwynethNode,
    GwynethPayloadAttributes, GwynethPayloadBuilderAttributes,
};
use alloy_consensus::Transaction;
use reth_chainspec::EthChainSpec;
use reth_consensus::Consensus;
use reth_db::DatabaseEnv;
use reth_ethereum_engine_primitives::EthPayloadAttributes;
use reth_evm_ethereum::EthEvmConfig;
use reth_execution_types::Chain;
use reth_exex::{ExExContext, ExExEvent};
use reth_node_api::{
    BuiltPayload, FullNodeComponents, FullNodeTypesAdapter,
    NodeTypesWithDBAdapter, PayloadBuilder, PayloadBuilderAttributes,
};
use reth_node_builder::{
    components::Components, rpc::RpcAddOns, FullNode, NodeAdapter, NodeComponents,
};
use reth_node_ethereum::{
    BasicBlockExecutorProvider, EthExecutionStrategyFactory,
};
use reth_payload_builder::{EthBuiltPayload, PayloadBuilderHandle};
use reth_provider::{
    providers::{BlockchainProvider, BlockchainProvider2},
    CanonStateSubscriptions, StateProvider, StateProviderFactory,
};
use reth_transaction_pool::{
    blobstore::DiskFileBlobStore, CoinbaseTipOrdering, EthPooledTransaction,
    EthTransactionValidator, Pool, TransactionValidationTaskExecutor,
};

sol!(RollupContract, "TaikoL1.json");
const ROLLUP_CONTRACT_ADDRESS: Address = address!("9fCF7D13d10dEdF17d0f24C62f0cf4ED462f65b7");

type GwynethProvider1 = BlockchainProvider<NodeTypesWithDBAdapter<GwynethNode, Arc<DatabaseEnv>>>;
type GwynethProvider2 = BlockchainProvider2<NodeTypesWithDBAdapter<GwynethNode, Arc<DatabaseEnv>>>;

type NodeDapter1 = NodeAdapter<
    FullNodeTypesAdapter<NodeTypesWithDBAdapter<GwynethNode, Arc<DatabaseEnv>>, GwynethProvider1>,
    Components<
        FullNodeTypesAdapter<
            NodeTypesWithDBAdapter<GwynethNode, Arc<DatabaseEnv>>,
            GwynethProvider1,
        >,
        Pool<
            TransactionValidationTaskExecutor<
                EthTransactionValidator<GwynethProvider1, EthPooledTransaction>,
            >,
            CoinbaseTipOrdering<EthPooledTransaction>,
            DiskFileBlobStore,
        >,
        EthEvmConfig,
        BasicBlockExecutorProvider<EthExecutionStrategyFactory>,
        Arc<dyn Consensus>,
    >,
>;

pub type GwynethFullNode1 = FullNode<
    NodeDapter1,
    RpcAddOns<
        NodeDapter1,
        EthApi<
            GwynethProvider1,
            Pool<
                TransactionValidationTaskExecutor<
                    EthTransactionValidator<GwynethProvider1, EthPooledTransaction>,
                >,
                CoinbaseTipOrdering<EthPooledTransaction>,
                DiskFileBlobStore,
            >,
            NetworkHandle,
            EthEvmConfig,
        >,
        GwynethEngineValidatorBuilder,
    >,
>;

type NodeDapter2 = NodeAdapter<
    FullNodeTypesAdapter<NodeTypesWithDBAdapter<GwynethNode, Arc<DatabaseEnv>>, GwynethProvider2>,
    Components<
        FullNodeTypesAdapter<
            NodeTypesWithDBAdapter<GwynethNode, Arc<DatabaseEnv>>,
            GwynethProvider2,
        >,
        Pool<
            TransactionValidationTaskExecutor<
                EthTransactionValidator<GwynethProvider2, EthPooledTransaction>,
            >,
            CoinbaseTipOrdering<EthPooledTransaction>,
            DiskFileBlobStore,
        >,
        EthEvmConfig,
        BasicBlockExecutorProvider<EthExecutionStrategyFactory>,
        Arc<dyn Consensus>,
    >,
>;

pub type GwynethFullNode2 = FullNode<
    NodeDapter2,
    RpcAddOns<
        NodeDapter2,
        EthApi<
            GwynethProvider2,
            Pool<
                TransactionValidationTaskExecutor<
                    EthTransactionValidator<GwynethProvider2, EthPooledTransaction>,
                >,
                CoinbaseTipOrdering<EthPooledTransaction>,
                DiskFileBlobStore,
            >,
            NetworkHandle,
            EthEvmConfig,
        >,
        GwynethEngineValidatorBuilder,
    >,
>;

pub enum GwynethFullNode {
    Provider1(GwynethFullNode1),
    Provider2(GwynethFullNode2),
}

impl GwynethFullNode {
    pub fn chain_id(&self) -> u64 {
        match self {
            GwynethFullNode::Provider1(node) => node.chain_spec().chain().id(),
            GwynethFullNode::Provider2(node) => node.chain_spec().chain().id(),
        }
    }

    pub fn payload_builder(&self) -> &PayloadBuilderHandle<GwynethEngineTypes> {
        match self {
            GwynethFullNode::Provider1(node) => &node.payload_builder,
            GwynethFullNode::Provider2(node) => &node.payload_builder,
        }
    }
}

#[derive(Debug)]
pub struct L1ParentState {
    pub block_number: u64,
    // None at launch
    pub header: Option<SealedHeader>,
}

#[derive(Clone, Debug, Default)]
pub struct L1ParentStates(Arc<HashMap<u64, RwLock<L1ParentState>>>);

impl L1ParentStates {
    pub fn new(nodes: &Vec<GwynethFullNode>) -> Self {
        let states = nodes
            .iter()
            .map(|node| {
                let chain_id = node.chain_id();
                let state = RwLock::new(L1ParentState { block_number: 0, header: None });
                (chain_id, state)
            })
            .collect::<HashMap<_, _>>();
        L1ParentStates(Arc::new(states))
    }

    pub fn get(&self, chain_id: u64) -> (u64, Option<SealedHeader>) {
        let state = self
            .0
            .get(&chain_id)
            .expect("L1ParentStates: chain_id not found")
            .read()
            .expect("L1ParentStates lock poisoned");
        (state.block_number, state.header.clone())
    }

    pub async fn update(&self, block: &SealedBlockWithSenders, chain_id: u64) -> eyre::Result<()> {
        let mut state = self
            .0
            .get(&chain_id)
            .expect("L1ParentStates: chain_id not found")
            .write()
            .expect("L1ParentStates lock poisoned");
        state.block_number = block.number;
        state.header = Some(block.header.clone());
        Ok(())
    }
}

pub struct Rollup<Node: reth_node_api::FullNodeComponents> {
    ctx: ExExContext<Node>,
    nodes: Vec<GwynethFullNode>,
    engine_apis: Vec<EngineApiContext<GwynethEngineTypes>>,
    pub l1_parents: L1ParentStates,
}

impl<Node: reth_node_api::FullNodeComponents> Rollup<Node> {
    pub fn new(
        ctx: ExExContext<Node>,
        nodes: Vec<GwynethFullNode>,
        l1_parents: L1ParentStates,
    ) -> eyre::Result<Self> {
        let mut engine_apis = Vec::new();
        for node in &nodes {
            match node {
                GwynethFullNode::Provider1(node) => {
                    let engine_api = EngineApiContext {
                        engine_api_client: node.auth_server_handle().http_client(),
                        canonical_stream: node.provider.canonical_state_stream(),
                        _marker: PhantomData::<GwynethEngineTypes>,
                    };
                    engine_apis.push(engine_api);
                }
                GwynethFullNode::Provider2(node) => {
                    let engine_api = EngineApiContext {
                        engine_api_client: node.auth_server_handle().http_client(),
                        canonical_stream: node.provider.canonical_state_stream(),
                        _marker: PhantomData::<GwynethEngineTypes>,
                    };
                    engine_apis.push(engine_api);
                }
            }
        }
        Ok(Self { ctx, nodes, engine_apis, l1_parents })
    }

    pub async fn start(mut self) -> eyre::Result<()> {
        while let Some(notification) = self.ctx.notifications.try_next().await? {
            if let Some(reverted_chain) = notification.reverted_chain() {
                self.revert(&reverted_chain)?;
            }

            if let Some(committed_chain) = notification.committed_chain() {
                println!(
                    "[reth] Exex Gwyneth: synced_l1_header 🎃 {:?}, synced_l1_number: {:?}",
                    committed_chain.tip().hash(),
                    committed_chain.tip().number
                );
                for (i, node) in self.nodes.iter().enumerate() {
                    self.commit(&committed_chain, i).await?;
                    self.l1_parents.update(committed_chain.tip(), node.chain_id()).await?;
                }
                let numhash =
                    BlockNumHash::new(committed_chain.tip().number, committed_chain.tip().hash());
                self.ctx.events.send(ExExEvent::FinishedHeight(numhash))?;
            }
        }

        Ok(())
    }

    pub async fn commit(&self, chain: &Chain, node_idx: usize) -> eyre::Result<()> {
        let node = &self.nodes[node_idx];
        let events = decode_chain_into_rollup_events(chain);
        for (block, _, event) in events {
            if let RollupContractEvents::BlockProposed(BlockProposed {
                blockId: block_number,
                meta,
            }) = event
            {
                println!("[reth] l2 {} l1 block_number: {:?}", node.chain_id(), block_number);
                let transactions: Vec<TransactionSigned> = decode_transactions(&meta.txList);
                println!("tx_list 🎉 : {:?}", transactions.len());

                let filtered_transactions: Vec<TransactionSigned> = transactions
                    .into_iter()
                    .filter(|tx| tx.chain_id() == Some(node.chain_id()))
                    .collect();

                if filtered_transactions.is_empty() {
                    self.l1_parents.update(block, node.chain_id()).await?;
                    println!("no transactions for chain: {}", node.chain_id());
                    continue;
                }

                let attrs = GwynethPayloadAttributes {
                    inner: EthPayloadAttributes {
                        timestamp: block.timestamp,
                        prev_randao: B256::ZERO,
                        suggested_fee_recipient: Address::ZERO,
                        withdrawals: Some(vec![]),
                        parent_beacon_block_root: Some(B256::ZERO),
                    },
                    transactions: Some(filtered_transactions.clone()),
                    gas_limit: None,
                };

                let l1_state_provider: Box<dyn StateProvider> = Box::new(
                    self.ctx
                        .provider()
                        .history_by_block_number(block_number.try_into().unwrap())
                        .unwrap(),
                );
                let sync_provider = HashMap::from([(
                    self.ctx.config.chain.chain().id(),
                    Arc::new(l1_state_provider),
                )]);

                let mut builder_attrs =
                    GwynethPayloadBuilderAttributes::try_new(B256::ZERO, attrs, 0).unwrap();
                builder_attrs.sync_provider = Some(sync_provider);

                let payload_id = builder_attrs.inner.payload_id();
                let parrent_beacon_block_root =
                    builder_attrs.inner.parent_beacon_block_root.unwrap();

                println!(
                    "👛 Exex: sending payload_id: {:?}\n tx {:?}",
                    payload_id,
                    builder_attrs.transactions.len()
                );

                // trigger new payload building draining the pool
                node.payload_builder().send_new_payload(builder_attrs).await.unwrap()?;

                // wait for the payload builder to have finished building
                let mut payload = EthBuiltPayload::new(
                    payload_id,
                    Arc::new(SealedBlock::default()),
                    U256::ZERO,
                    None,
                    None,
                );
                loop {
                    let result = node.payload_builder().best_payload(payload_id).await;

                    if let Some(result) = result {
                        if let Ok(new_payload) = result {
                            payload = new_payload;
                            if payload.block().body.transactions.is_empty() {
                                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                                continue;
                            }
                        } else {
                            println!("Gwyneth: No payload?");
                            continue;
                        }
                    } else {
                        println!("Gwyneth: No block?");
                        continue;
                    }
                    break;
                }

                // trigger resolve payload via engine api
                self.engine_apis[node_idx].get_payload_v3_value(payload_id).await?;

                // submit payload to engine api
                let block_hash = self.engine_apis[node_idx]
                    .submit_payload(
                        payload.clone(),
                        parrent_beacon_block_root,
                        PayloadStatusEnum::Valid,
                        vec![],
                    )
                    .await?;

                // trigger forkchoice update via engine api to commit the block to the blockchain
                self.engine_apis[node_idx].update_forkchoice(block_hash, block_hash).await?;
            }
            self.l1_parents.update(block, node.chain_id()).await?;
        }
        Ok(())
    }

    fn revert(&mut self, chain: &Chain) -> eyre::Result<()> {
        unimplemented!()
    }
}

/// Decode chain of blocks into a flattened list of receipt logs, filter only transactions to the
/// Rollup contract [`ROLLUP_CONTRACT_ADDRESS`] and extract [`RollupContractEvents`].
fn decode_chain_into_rollup_events(
    chain: &Chain,
) -> Vec<(&SealedBlockWithSenders, &TransactionSigned, RollupContractEvents)> {
    chain
        // Get all blocks and receipts
        .blocks_and_receipts()
        // Get all receipts
        .flat_map(|(block, receipts)| {
            block
                .body
                .transactions
                .iter()
                .zip(receipts.iter().flatten())
                .map(move |(tx, receipt)| (block, tx, receipt))
        })
        // Get all logs from rollup contract
        .flat_map(|(block, tx, receipt)| {
            receipt
                .logs
                .iter()
                .filter(|log| log.address == ROLLUP_CONTRACT_ADDRESS)
                .map(move |log| (block, tx, log))
        })
        // Decode and filter rollup events
        .filter_map(|(block, tx, log)| {
            RollupContractEvents::decode_raw_log(log.topics(), &log.data.data, true)
                .ok()
                .map(|event| (block, tx, event))
        })
        .collect()
}

fn decode_transactions(tx_list: &[u8]) -> Vec<TransactionSigned> {
    #[allow(clippy::useless_asref)]
    Vec::<TransactionSigned>::decode(&mut tx_list.as_ref()).unwrap_or_else(|e| {
        // If decoding fails we need to make an empty block
        println!("decode_transactions not successful: {e:?}, use empty tx_list");
        vec![]
    })
}
