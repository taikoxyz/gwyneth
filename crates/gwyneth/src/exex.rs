use std::{collections::HashMap, marker::PhantomData, sync::Arc};

use alloy_rlp::Decodable;
use alloy_sol_types::{sol, SolEventInterface};
use reth_chainspec::EthChainSpec;

use crate::{
    engine_api::EngineApiContext, GwynethEngineTypes, GwynethNode, GwynethPayloadAttributes,
    GwynethPayloadBuilderAttributes,
};
use reth_consensus::Consensus;
use reth_db::{test_utils::TempDatabase, DatabaseEnv};
use reth_ethereum_engine_primitives::EthPayloadAttributes;
use reth_evm_ethereum::EthEvmConfig;
use reth_execution_types::Chain;
use reth_exex::{ExExContext, ExExEvent};
use reth_node_api::{FullNodeTypesAdapter, PayloadBuilderAttributes};
use reth_node_builder::{components::Components, FullNode, Node, NodeAdapter};
use reth_node_ethereum::{node::EthereumAddOns, EthExecutorProvider};
use reth_payload_builder::EthBuiltPayload;
use reth_provider::{
    providers::BlockchainProvider, BlockNumReader, CanonStateSubscriptions, DatabaseProviderFactory,
};
use reth_transaction_pool::{
    blobstore::DiskFileBlobStore, CoinbaseTipOrdering, EthPooledTransaction,
    EthTransactionValidator, Pool, TransactionValidationTaskExecutor,
};
use RollupContract::{BlockProposed, RollupContractEvents};
use reth_provider::BlockReaderIdExt;
use alloy_primitives::{address, b256, Address, BlockNumber, B256, Bytes, U256};
use reth_node_core::primitives::TransactionSigned;
use reth_primitives::{ChainDA, GwynethDA, SealedBlock, SealedBlockWithSenders};
use alloy_rpc_types_engine::PayloadStatusEnum;
use reth_node_builder::rpc::RethRpcAddOns;
use tracing::info;
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use reth_node_builder::rpc::EngineValidatorBuilder;
use reth_node_api::PayloadBuilder;
use reth_node_api::FullNodeComponents;
use reth_node_api::NodeTypesWithDBAdapter;
use reth_node_ethereum::BasicBlockExecutorProvider;
use reth_evm_ethereum::execute::EthExecutionStrategyFactory;
use futures_util::TryStreamExt;
use futures_util::FutureExt;
use reth_node_api::NodeTypes;
use reth_primitives::EthPrimitives;
use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_node_ethereum::EthEngineTypes;

const ROLLUP_CONTRACT_ADDRESS: Address = address!("9fCF7D13d10dEdF17d0f24C62f0cf4ED462f65b7");
pub const BASE_CHAIN_ID: u64 = 167010;
const INITIAL_TIMESTAMP: u64 = 1710338135;

pub type GwynethNodeAdapter = NodeAdapter<
    FullNodeTypesAdapter<
        NodeTypesWithDBAdapter<GwynethNode,Arc<DatabaseEnv>>,
        BlockchainProvider<
            NodeTypesWithDBAdapter<GwynethNode,Arc<DatabaseEnv>>
        >
    >,
    Components<
        FullNodeTypesAdapter<
            NodeTypesWithDBAdapter<GwynethNode, Arc<DatabaseEnv>>,
            BlockchainProvider<
                NodeTypesWithDBAdapter<GwynethNode, Arc<DatabaseEnv>>
            >
        >,
        Pool<
            TransactionValidationTaskExecutor<
                EthTransactionValidator<
                    BlockchainProvider<
                        NodeTypesWithDBAdapter<GwynethNode, Arc<DatabaseEnv>>
                    >,
                    EthPooledTransaction,
                >,
            >,
            CoinbaseTipOrdering<EthPooledTransaction>,
            DiskFileBlobStore,
        >,
        EthEvmConfig,
        BasicBlockExecutorProvider<EthExecutionStrategyFactory>,
        Arc<dyn Consensus>,
    >,
>;

pub type GwynethFullNode =  FullNode<
    GwynethNodeAdapter,
    EthereumAddOns<GwynethNodeAdapter>,
>;

// pub type GwynethFullNode = FullNode<
//     NodeAdapter<
//         FullNodeTypesAdapter<
//             NodeTypesWithDBAdapter<EthereumNode,Arc<DatabaseEnv>>,
//             BlockchainProvider<
//                 NodeTypesWithDBAdapter<EthereumNode,Arc<DatabaseEnv>>
//             >
//         >,
//         Components<
//             FullNodeTypesAdapter<
//                 NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>,
//                 BlockchainProvider<
//                     NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>
//                 >
//             >,
//             Pool<
//                 TransactionValidationTaskExecutor<
//                     EthTransactionValidator<
//                         BlockchainProvider<
//                             NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>
//                         >,
//                         EthPooledTransaction,
//                     >,
//                 >,
//                 CoinbaseTipOrdering<EthPooledTransaction>,
//                 DiskFileBlobStore,
//             >,
//             EthEvmConfig,
//             BasicBlockExecutorProvider<EthExecutionStrategyFactory>,
//             Arc<dyn Consensus>,
//         >,
//     >,
//     GwynethAddOns<GwynethNode>,
// >;


// pub type GwynethFullNode = FullNode<
//     NodeAdapter<
//         FullNodeTypesAdapter<
//             NodeTypesWithDBAdapter<GwynethNode, Arc<DatabaseEnv>>,
//             BlockchainProvider<
//                 NodeTypesWithDBAdapter<GwynethNode, Arc<DatabaseEnv>>,
//             >,
//         >,
//         Components<
//             FullNodeTypesAdapter<
//                 NodeTypesWithDBAdapter<GwynethNode, Arc<DatabaseEnv>>,
//                 BlockchainProvider<
//                     NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>
//                 >,
//             >,
//             Pool<
//                 TransactionValidationTaskExecutor<
//                     EthTransactionValidator<
//                         BlockchainProvider<Arc<DatabaseEnv>>,
//                         EthPooledTransaction,
//                     >,
//                 >,
//                 CoinbaseTipOrdering<EthPooledTransaction>,
//                 DiskFileBlobStore,
//             >,
//             EthEvmConfig,
//             EthExecutorProvider,
//             Arc<dyn Consensus>,
//         >,
//     >,
//     EthereumAddOns,
// >;


// FullNodeTypesAdapter<
//             NodeTypesWithDBAdapter<TestNode, TmpDB>,
//             BlockchainProvider<NodeTypesWithDBAdapter<TestNode, TmpDB>>,
//         >,

// pub type GwynethFullNode = FullNode<
//     NodeAdapter<
//         FullNodeTypesAdapter<
//             GwynethNode,
//             Arc<DatabaseEnv>,
//             BlockchainProvider<Arc<DatabaseEnv>>,
//         >,
//         Components<
//             FullNodeTypesAdapter<
//                 GwynethNode,
//                 Arc<DatabaseEnv>,
//                 BlockchainProvider<Arc<DatabaseEnv>>,
//             >,
//             Pool<
//                 TransactionValidationTaskExecutor<
//                     EthTransactionValidator<
//                         BlockchainProvider<Arc<DatabaseEnv>>,
//                         EthPooledTransaction,
//                     >,
//                 >,
//                 CoinbaseTipOrdering<EthPooledTransaction>,
//                 DiskFileBlobStore,
//             >,
//             EthEvmConfig,
//             EthExecutorProvider,
//             Arc<dyn Consensus>,
//         >,
//     >,
//     EthereumAddOns,
// >;


//pub type GwynethFullNode = ();

//pub type GwynethFullNode = FullNode<GwynethNode, GwynethAddOns<GwynethNode>>;

sol!(RollupContract, "TaikoL1.json");

pub struct Rollup<Node: FullNodeComponents> {
    ctx: ExExContext<Node>,
    nodes: Vec<GwynethFullNode>,
    engine_apis: Vec<EngineApiContext<EthEngineTypes>>, // TODO
    num_l2_blocks: u64,
}

impl<Node> Rollup<Node>
where
    Node: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>,
{
    pub async fn new(ctx: ExExContext<Node>, nodes: Vec<GwynethFullNode>) -> eyre::Result<Self> {
        let mut engine_apis = Vec::new();
        for node in &nodes {
            let engine_api = EngineApiContext {
                engine_api_client: node.auth_server_handle().http_client(),
                canonical_stream: node.provider.canonical_state_stream(),
                _marker: PhantomData::<EthEngineTypes>,
            };
            engine_apis.push(engine_api);
        }
        Ok(Self { ctx, nodes, /* payload_event_stream, */ engine_apis, num_l2_blocks: 0 })
    }

    pub async fn commit(&mut self, chain: &Chain, node_idx: usize) -> eyre::Result<()> {
        let events = decode_chain_into_rollup_events(chain);

        // Add all other L2 dbs for now as well until dependencies are broken
        // let mut last_block_number = HashMap::new();
        // for node in self.nodes.iter() {
        //     let chain_id = node.config.chain.chain().id();
        //     let state_provider = node
        //                     .provider
        //                     .database_provider_ro()
        //                     .unwrap();
        //     last_block_number.insert(chain_id, state_provider.last_block_number()?);
        // }

        for (block, _, event) in events {
            if let RollupContractEvents::BlockProposed(BlockProposed {
                blockId: block_number,
                meta,
            }) = event
            {
                println!("block_number: {:?}", block_number);
                println!("block hash: {:?}", meta.blockHash);
                println!("tx_list: {:?}", meta.txList);
                println!("state diffs: {:?}", meta.stateDiffs);
                //println!("L1 state diff: {:?}", meta.l1StateDiff.);
                let transactions: Vec<TransactionSigned> = decode_transactions(&meta.txList);
                println!("transactions: {:?}", transactions);

                let da: GwynethDA = bincode::deserialize(&meta.stateDiffs.to_vec()).unwrap_or_else(|err| {
                    panic!("DA can't be decoded: {}", err);
                });
                println!("da: {:?}", da);

                let all_transactions: Vec<TransactionSigned> = decode_transactions(&meta.txList);
                let node_chain_id = BASE_CHAIN_ID + (node_idx as u64);

                let chain_da = da.chain_das.get(&node_chain_id);
                if chain_da.is_none() {
                    println!("No block for {}", node_chain_id);
                    continue;
                } else {
                    println!("New block for {}!", node_chain_id);
                }
                let default_chain_da = ChainDA {
                    block_hash: B256::default(),
                    extra_data: Bytes::new(),
                    state_diff: None,
                    transactions: None,
                };
                let chain_da = chain_da.unwrap_or(&default_chain_da);
                //println!("chain_da: {:?}", chain_da);

                // let filtered_transactions: Vec<TransactionSigned> = all_transactions
                //     .into_iter()
                //     .filter(|tx| tx.chain_id() == Some(node_chain_id))
                //     .collect();

                // if filtered_transactions.len() == 0 {
                //     println!("no transactions for chain: {}", node_chain_id);
                //     continue;
                // }

                let filtered_transactions: Vec<TransactionSigned> = all_transactions;

                let attrs = GwynethPayloadAttributes {
                    inner: EthPayloadAttributes {
                        timestamp: block.timestamp,
                        prev_randao: /*block.mix_hash*/ B256::ZERO,
                        suggested_fee_recipient: meta.coinbase,
                        withdrawals: Some(vec![]),
                        parent_beacon_block_root: /*block.parent_beacon_block_root*/ Some(B256::ZERO),
                    },
                    transactions: Some(filtered_transactions.clone()),
                    chain_da: chain_da.clone(),
                    gas_limit: None,
                };

                // let l1_state_provider = self
                //     .ctx
                //     .provider()
                //     .database_provider_ro()
                //     .unwrap()
                //     .state_provider_by_block_number(block.number)
                //     .unwrap();

                let mut builder_attrs =
                    GwynethPayloadBuilderAttributes::try_new(B256::ZERO, attrs, 0).unwrap();


                let mut builder_attrs = EthPayloadBuilderAttributes::default();

                //builder_attrs.providers.insert(self.ctx.config.chain.chain_id(), Arc::new(l1_state_provider));

                // Add all other L2 dbs for now as well until dependencies are broken
                // for node in self.nodes.iter() {
                //     let chain_id = node.config.chain.chain().id();
                //     println!("other chain_id: {}", chain_id);
                //     if chain_id != node_chain_id {
                //         println!("Adding chain_id: {}", chain_id);
                //         let state_provider = node
                //             .provider
                //             .database_provider_ro()
                //             .unwrap();
                //         //let last_block_number = state_provider.last_block_number()?;
                //         //let last_block_number = *last_block_number.get(&chain_id).unwrap();
                //         //println!("last block number: {} -> {}", chain_id, last_block_number);
                //         let last_block_number = self.num_l2_blocks / self.nodes.len() as u64;
                //         println!("exex executing against {}", last_block_number);
                //         let state_provider = state_provider.state_provider_by_block_number(last_block_number).unwrap();

                //         builder_attrs.providers.insert(chain_id, Arc::new(state_provider));
                //     }
                // }

                //let payload_id = builder_attrs.inner.payload_id();
                // let parrent_beacon_block_root =
                //     builder_attrs.inner.parent_beacon_block_root.unwrap();
                let payload_id = builder_attrs.payload_id();
                let parrent_beacon_block_root =
                    builder_attrs.parent_beacon_block_root.unwrap();

                //println!("payload_id: {} {}", node_idx, payload_id);

                // trigger new payload building draining the pool
                // self.nodes[node_idx].payload_builder().send_new_payload(builder_attrs).await.unwrap().expect("new payload");

                // // wait for the payload builder to have finished building
                // let mut payload =
                //     EthBuiltPayload::new(payload_id, Arc::new(SealedBlock::default()), U256::ZERO, None, None);
                // loop {
                //     let result = self.nodes[node_idx].payload_builder().best_payload(payload_id).await;

                //     if let Some(result) = result {
                //         if let Ok(new_payload) = result {
                //             payload = new_payload;
                //             // TODO(Brecht): ah no empty blocks
                //             // if payload.block().body.is_empty() {
                //             //     tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                //             //     continue;
                //             // }
                //         } else {
                //             println!("Gwyneth: No payload?");
                //             continue;
                //         }
                //     } else {
                //         println!("Gwyneth: No block for {}?", node_chain_id);
                //         tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                //         continue;
                //     }
                //     break;
                // }

                // trigger resolve payload via engine api
                // self.engine_apis[node_idx].get_payload_v3_value(payload_id).await?;

                // // submit payload to engine api
                // let block_hash = self.engine_apis[node_idx]
                //     .submit_payload(
                //         payload.clone(),
                //         parrent_beacon_block_root,
                //         PayloadStatusEnum::Valid,
                //         vec![],
                //     )
                //     .await?;


                // if chain_da.block_hash != B256::ZERO {
                //     assert_eq!(block_hash, chain_da.block_hash, "unexpected block hash for chain {} block {}", node_chain_id, payload.block().number);
                // }

                // // trigger forkchoice update via engine api to commit the block to the blockchain
                // self.engine_apis[node_idx].update_forkchoice(block_hash, block_hash).await?;

                // loop {
                //     // wait for the block to commit
                //     tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                //     if let Some(latest_block) =
                //         self.nodes[node_idx].provider.block_by_number_or_tag(BlockNumberOrTag::Latest)?
                //     {
                //         if latest_block.number == payload.block().number {
                //             // make sure the block hash we submitted via FCU engine api is the new latest
                //             // block using an RPC call
                //             assert_eq!(latest_block.hash_slow(), block_hash);
                //             break
                //         }
                //     }
                //     println!("waiting on L2 block for {}: {}", node_chain_id, payload.block().number)
                // }

                // println!("[L1 block {}] Done with block {}: {}", block.number, node_chain_id, payload.block().number);

                self.num_l2_blocks += 1;
            }
        }

        Ok(())
    }

    fn revert(&mut self, chain: &Chain) -> eyre::Result<()> {
        unimplemented!()
    }
}

// impl<Node: FullNodeComponents> Future for Rollup<Node> {
//     type Output = eyre::Result<()>;

//     fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
//         let this = self.get_mut();

//         while let Some(notification) = ready!(this.ctx.notifications.try_next().poll_unpin(cx))? {
//             if let Some(reverted_chain) = notification.reverted_chain() {
//                 this.revert(&reverted_chain)?;
//             }

//             if let Some(committed_chain) = notification.committed_chain() {
//                 println!("EXEX called for block {}", committed_chain.tip().number);
//                 for i in 0..this.nodes.len() {
//                     this.commit(&committed_chain, i)?;
//                 }
//                 this.ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().num_hash()))?;
//             }

//             // if let Some(first_block) = this.first_block {
//             //     info!(%first_block, transactions = %this.transactions, "Total number of transactions");
//             // }
//         }

//         Poll::Ready(Ok(()))
//     }
// }

impl<Node> Rollup<Node>
where
    Node: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>,
{
    // fn new(ctx: ExExContext<Node>, connection: Connection) -> eyre::Result<Self> {
    //     let db = Database::new(connection)?;
    //     Ok(Self { ctx, db })
    // }

    pub async fn start(mut self) -> eyre::Result<()> {
        // Process all new chain state notifications
        while let Some(notification) = self.ctx.notifications.try_next().await? {
            if let Some(reverted_chain) = notification.reverted_chain() {
                self.revert(&reverted_chain)?;
            }

            if let Some(committed_chain) = notification.committed_chain() {
                println!("EXEX called for block {}", committed_chain.tip().number);
                for i in 0..self.nodes.len() {
                    self.commit(&committed_chain, i).await?;
                }
                self.ctx
                    .events
                    .send(ExExEvent::FinishedHeight(committed_chain.tip().num_hash()))?;
            }
        }

        Ok(())
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
                .filter(|log| {
                    log.address == ROLLUP_CONTRACT_ADDRESS
                })
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
