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
    EthTransactionValidator, Pool, TransactionValidationTaskExecutor, TransactionPool,
};

sol!(RollupContract, "TaikoL1.json");
const ROLLUP_CONTRACT_ADDRESS: Address = address!("9fCF7D13d10dEdF17d0f24C62f0cf4ED462f65b7");
pub const BASE_CHAIN_ID: u64 = 167010;
const FINALIZATION_PERIOD: u64 = 64;
const GENESIS_HASH: B256 = b256!("930c04603408ca90f730fb9ad792af0d42bd01db97633df8f9f58330cf103b0c");

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


pub struct Rollup<Node: reth_node_api::FullNodeComponents> {
    ctx: ExExContext<Node>,
    nodes: Vec<GwynethFullNode>,
    engine_apis: Vec<EngineApiContext<GwynethEngineTypes>>,
    l1_to_l2: HashMap<u64, HashMap<u64, (u64, B256)>>,
    l1_finalized_block: u64,
}

impl<Node: reth_node_api::FullNodeComponents> Rollup<Node> {
    pub fn new(
        ctx: ExExContext<Node>,
        nodes: Vec<GwynethFullNode>,
        l1_parents: L1ParentStates,
    ) -> eyre::Result<Self> {
        let mut engine_apis = Vec::new();
        let mut l2_block_info = HashMap::new();
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
            let chain_id = node.chain_spec().chain().id();
            let mut l2_block_indices = GWYNETH_SYNCED_L2_BLOCK_IDX.lock().unwrap();
            l2_block_indices.insert(chain_id, 0);
            l2_block_info.insert(chain_id, (0u64, GENESIS_HASH));
        }
        let l1_finalized_block = ctx.head.number;
        let mut l1_to_l2 = HashMap::new();
        l1_to_l2.insert(l1_finalized_block, l2_block_info);

        Ok(Self {
            ctx,
            nodes,
            engine_apis,
            l1_to_l2,
            l1_finalized_block,
        })
    }

    pub async fn start(mut self) -> eyre::Result<()> {
        while let Some(notification) = self.ctx.notifications.try_next().await? {
            if let Some(reverted_chain) = notification.reverted_chain() {
                // Find the oldest L1 block number (and subtract 1) in the given chain to get the newest latest L1 block
                let target_l1_block = reverted_chain.blocks().keys().min().copied()
                    .ok_or_else(|| eyre::eyre!("Chain is empty"))?
                    .saturating_sub(1);

                println!("REORG!!! Reverting to {}", target_l1_block);

                // for i in 0..self.nodes.len() {
                //     self.revert(i, target_l1_block).await?;
                // }

                // Update the sync data
                unsafe {
                    GWYNETH_SYNCED_L1_BLOCK_IDX = target_l1_block;
                    println!("Updated L1 sync data: {}", GWYNETH_SYNCED_L1_BLOCK_IDX);
                }
            }

            if let Some(committed_chain) = notification.committed_chain() {
                println!(
                    "[reth] Exex Gwyneth: synced_l1_header 🎃 {:?}, synced_l1_number: {:?}",
                    committed_chain.tip().hash(),
                    committed_chain.tip().number
                );

                for block in committed_chain.blocks_iter() {
                    self.l1_to_l2.insert(block.number, self.l1_to_l2.get(&(block.number - 1)).unwrap().clone());
                }
                

                for (i, node) in self.nodes.iter().enumerate() {
                    self.commit(&committed_chain, i).await?;
                }
                // Update the sync data
                unsafe {
                    GWYNETH_SYNCED_L1_BLOCK_IDX = committed_chain.tip().number;
                    println!("Updated L1 sync data: {}", GWYNETH_SYNCED_L1_BLOCK_IDX);
                }
                let numhash = BlockNumHash::new(committed_chain.tip().number, committed_chain.tip().hash());
                self.ctx.events.send(ExExEvent::FinishedHeight(numhash))?;
            }
        }

        Ok(())
    }

    // TODO(Brecht): handle block by block instead of chain (easier to manage reorg stuff)
    pub async fn commit(&mut self, chain: &Chain, node_idx: usize) -> eyre::Result<()> {
        let events = decode_chain_into_rollup_events(chain);
        for (block, tx, event) in events {
            if let RollupContractEvents::BlockProposed(BlockProposed {
                block: ultra_block,
            }) = event
            {
                println!("[reth] l2 {} l1 block_number: {:?}", node.chain_id(), block_number);
                let transactions: Vec<TransactionSigned> = decode_transactions(&meta.txList);
                println!("tx_list 🎉 : {:?}", transactions.len());

                let (da, tx_list) = bincode::deserialize::<(GwynethDA, Vec<u8>)>(&ultra_block.da)
                .unwrap_or_else(|err| {
                    panic!("DA can't be decoded: {}", err);
                });

                let node_chain_id = self.get_chain_id(node_idx);
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


                let attrs = GwynethPayloadAttributes {
                    inner: EthPayloadAttributes {
                        timestamp: block.timestamp,
                        prev_randao: block.mix_hash,
                        suggested_fee_recipient: ultra_block.blocks[0].coinbase,
                        withdrawals: Some(vec![]),
                        parent_beacon_block_root: block.parent_beacon_block_root,
                    },
                    transactions: Some(filtered_transactions.clone()),
                    chain_da: chain_da.clone(),
                    gas_limit: None,
                };

                let l1_state_provider: Box<dyn StateProvider> = Box::new(
                    self.ctx
                        .provider()
                        .history_by_block_number(block_number.try_into().unwrap())
                        .unwrap(),
                );
                let l1_block_number = block.number;


                let mut builder_attrs =
                    GwynethPayloadBuilderAttributes::try_new(B256::ZERO, attrs, 0).unwrap();
                builder_attrs.providers.insert(self.ctx.config.chain.chain().id(), Arc::new(l1_state_provider));

                let payload_id = builder_attrs.inner.payload_id();
                let parrent_beacon_block_root =
                    builder_attrs.inner.parent_beacon_block_root.unwrap();

                println!(
                    "👛 Exex: sending payload_id: {:?}\n tx {:?}",
                    payload_id,
                    builder_attrs.transactions.len()
                );

                // trigger new payload building draining the pool
                self.nodes[node_idx].payload_builder.new_payload(builder_attrs).await.unwrap();

                // wait for the payload builder to have finished building
                let mut payload =
                    EthBuiltPayload::new(payload_id, SealedBlock::default(), U256::ZERO);
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
                        println!("Gwyneth: No block for {}?", node_chain_id);
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        continue;
                    }
                    break;
                }

                // trigger resolve payload via engine api
                self.engine_apis[node_idx].get_payload_v3_value(payload_id).await?;

                //self.payloads.push(payload.clone());

                // submit payload to engine api
                let block_hash = self.engine_apis[node_idx]
                    .submit_payload(
                        payload.clone(),
                        parrent_beacon_block_root,
                        PayloadStatusEnum::Valid,
                        vec![],
                    )
                    .await?;

                if chain_da.block_hash != B256::ZERO {
                    if block_hash != chain_da.block_hash {
                        println!("da data for block: {:?}", chain_da);
                        println!("reth block: {:?}", payload.block());
                    }
                    assert_eq!(block_hash, chain_da.block_hash, "unexpected block hash for chain {} block {}", node_chain_id, payload.block().number);
                }

                // Determine the finalized block hash for this L2
                let finalized_hash = self.l1_to_l2.get(&self.l1_finalized_block).unwrap().get(&node_chain_id).unwrap().1;

                // println!("finalized hash: {}", finalized_hash);
                // println!("Block number: {}", payload.block().number);

                // trigger forkchoice update via engine api to commit the block to the blockchain
                self.engine_apis[node_idx].update_forkchoice(block_hash, block_hash).await?;




                println!("[L1 block {}] Done with block {}: {} ({}, finalized: {})", block.number, node_chain_id, payload.block().number, block_hash, finalized_hash);
                if payload.block().number == 1 {
                    assert_eq!(payload.block().parent_hash, GENESIS_HASH, "genesis hash is incorrect");
                }

                // For rbuilder syncing
                let mut l2_block_indices = GWYNETH_SYNCED_L2_BLOCK_IDX.lock().unwrap();
                l2_block_indices.insert(node_chain_id, payload.block().number);

                // To support reorgs
                self.l1_to_l2.get_mut(&l1_block_number).unwrap().insert(node_chain_id, (payload.block().number, block_hash));
            }
        }

        Ok(())
    }

    pub async fn revert(&mut self, node_idx: usize, target_l1_block: u64) -> eyre::Result<()> {
        let chain_id = self.get_chain_id(node_idx);

        // Get the finalized block and the last L2 block still included in the canonical L1 chain
        let finalized_block_hash = self.l1_to_l2.get(&self.l1_finalized_block).unwrap().get(&chain_id).unwrap().1;
        let newest_block = self.l1_to_l2.get(&target_l1_block).unwrap().get(&chain_id).unwrap();

        // Update forkchoice to revert back
        self.engine_apis[node_idx].update_forkchoice(finalized_block_hash, newest_block.1).await?;

        // Update the sync info
        let mut l2_block_indices = GWYNETH_SYNCED_L2_BLOCK_IDX.lock().unwrap();
        l2_block_indices.insert(chain_id, newest_block.0);

        Ok(())
    }

    fn get_chain_id(&self, node_idx: usize) -> u64 {
        //BASE_CHAIN_ID + (node_idx as u64)
        self.nodes[node_idx].chain_spec().chain().id()
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

fn get_blob_data<Pool: TransactionPool>(pool: &Pool, tx: &TransactionSigned, blob_hashes: Vec<B256>) -> eyre::Result<Vec<u8>> {
    let blobs: Vec<_> = if let Some(sidecar) = pool.get_blob(tx.hash())? {
        // Try to get blobs from the transaction pool
        sidecar.blobs.clone().into_iter().zip(sidecar.commitments.clone()).collect()
    } else {
        eyre::bail!("blobs not found for: {:?}", tx.hash())
    };

    // Filter blobs that are present in the block data
    let blobs = blobs
        .into_iter()
        // Convert blob KZG commitments to versioned hashes
        .map(|(blob, commitment)| (blob, kzg_to_versioned_hash(commitment.as_slice())))
        // Filter only blobs that are present in the block data
        .filter(|(_, hash)| blob_hashes.contains(hash))
        .map(|(blob, _)| Blob::from(*blob))
        .collect::<Vec<_>>();
    if blobs.len() != blob_hashes.len() {
        eyre::bail!("some blobs not found")
    }

    // Decode blobs and concatenate them to get the raw transactions
    let data = SimpleCoder::default()
        .decode_all(&blobs)
        .ok_or(eyre::eyre!("failed to decode blobs"))?
        .concat();

    Ok(data)
}
