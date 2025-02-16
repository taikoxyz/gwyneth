use std::{collections::{HashMap, VecDeque}, marker::PhantomData, sync::Arc};
use alloy_rlp::Decodable;
use alloy_sol_types::{sol, SolEventInterface};
use reth_network::NetworkInfo;
use reth_rpc_api::eth::helpers::EthApiSpec;

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
use reth_primitives::{
    address, Address, Bytes, ChainDA, GwynethDA, SealedBlock, SealedBlockWithSenders, StateDiff, TransactionSigned, B256, U256
};
use reth_provider::{
    providers::BlockchainProvider, BlockNumReader, CanonStateSubscriptions, DatabaseProviderFactory,
};
use reth_rpc_types::{engine::PayloadStatusEnum, BlockNumberOrTag};
use reth_transaction_pool::{
    blobstore::DiskFileBlobStore, CoinbaseTipOrdering, EthPooledTransaction,
    EthTransactionValidator, Pool, TransactionValidationTaskExecutor,
};
use RollupContract::{BlockProposed, RollupContractEvents};
use reth_provider::BlockReaderIdExt;
use reth_provider::{GWYNETH_SYNCED_L1_BLOCK_IDX, GWYNETH_SYNCED_L2_BLOCK_IDX};

const ROLLUP_CONTRACT_ADDRESS: Address = address!("9fCF7D13d10dEdF17d0f24C62f0cf4ED462f65b7");
pub const BASE_CHAIN_ID: u64 = 167010;
const INITIAL_TIMESTAMP: u64 = 1710338135;
const RING_BUFFER_SIZE: usize = 128;

const FINALIZATION_PERIOD: u64 = 64;
const GENESIS_HASH: B256 = B256::new([
    0x93, 0x0c, 0x04, 0x60, 0x34, 0x08, 0xca, 0x90,
    0xf7, 0x30, 0xfb, 0x9a, 0xd7, 0x92, 0xaf, 0x0d,
    0x42, 0xbd, 0x01, 0xdb, 0x97, 0x63, 0x3d, 0xf8,
    0xf9, 0xf5, 0x83, 0x30, 0xcf, 0x10, 0x3b, 0x0c
]);

#[derive(Clone, Debug)]
struct L1L2Mapping {
    l1_block: u64,
    l2_block: u64,
    l2_hash: B256,
}

pub type GwynethFullNode = FullNode<
    NodeAdapter<
        FullNodeTypesAdapter<
            GwynethNode,
            Arc<DatabaseEnv>,
            BlockchainProvider<Arc<DatabaseEnv>>,
        >,
        Components<
            FullNodeTypesAdapter<
                GwynethNode,
                Arc<DatabaseEnv>,
                BlockchainProvider<Arc<DatabaseEnv>>,
            >,
            Pool<
                TransactionValidationTaskExecutor<
                    EthTransactionValidator<
                        BlockchainProvider<Arc<DatabaseEnv>>,
                        EthPooledTransaction,
                    >,
                >,
                CoinbaseTipOrdering<EthPooledTransaction>,
                DiskFileBlobStore,
            >,
            EthEvmConfig,
            EthExecutorProvider,
            Arc<dyn Consensus>,
        >,
    >,
    EthereumAddOns,
>;

sol!(RollupContract, "Gwyneth.json");

pub struct Rollup<Node: reth_node_api::FullNodeComponents> {
    ctx: ExExContext<Node>,
    nodes: Vec<GwynethFullNode>,
    engine_apis: Vec<EngineApiContext<GwynethEngineTypes>>,
    num_l2_blocks: u64,
    l1_l2_ring_buffers: Vec<VecDeque<L1L2Mapping>>,
    l2_genesis_l1_block: u64,
}

impl<Node: reth_node_api::FullNodeComponents> Rollup<Node> {
    pub async fn new(ctx: ExExContext<Node>, nodes: Vec<GwynethFullNode>) -> eyre::Result<Self> {
        let mut engine_apis = Vec::new();
        let mut l1_l2_ring_buffers = Vec::new();
        for node in &nodes {
            let engine_api = EngineApiContext {
                engine_api_client: node.auth_server_handle().http_client(),
                canonical_stream: node.provider.canonical_state_stream(),
                _marker: PhantomData::<GwynethEngineTypes>,
            };
            engine_apis.push(engine_api);

            let mut l2_block_indices = GWYNETH_SYNCED_L2_BLOCK_IDX.lock().unwrap();
            l2_block_indices.insert(node.chain_spec().chain().id(), 0);

            l1_l2_ring_buffers.push(VecDeque::with_capacity(RING_BUFFER_SIZE));
        }

        Ok(Self {
            ctx,
            nodes,
            engine_apis,
            num_l2_blocks: 0,
            l1_l2_ring_buffers,
            l2_genesis_l1_block: 0,
        })
    }

    pub async fn start(mut self) -> eyre::Result<()> {
        while let Some(notification) = self.ctx.notifications.recv().await {
            if let Some(reverted_chain) = notification.reverted_chain() {
                for i in 0..self.nodes.len() {
                    self.revert(&reverted_chain, i).await?;
                }
            }

            if let Some(committed_chain) = notification.committed_chain() {
                println!("EXEX called for block {}", committed_chain.tip().number);
                for i in 0..self.nodes.len() {
                    self.commit(&committed_chain, i).await?;
                }

                // Update the sync data
                unsafe {
                    GWYNETH_SYNCED_L1_BLOCK_IDX = committed_chain.tip().number;
                    println!("Updated L1 sync data: {}", GWYNETH_SYNCED_L1_BLOCK_IDX);
                }

                self.ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().number))?;
            }
        }

        Ok(())
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
                //println!("tx_list: {:?}", meta.txList);
                //println!("state diffs: {:?}", meta.stateDiffs);
                //println!("L1 state diff: {:?}", meta.l1StateDiff.);

                let transactions: Vec<TransactionSigned> = decode_transactions(&meta.txList);
                println!("transactions: {:?}", transactions.len());

                let da: GwynethDA = bincode::deserialize(&meta.stateDiffs.to_vec()).unwrap_or_else(|err| {
                    panic!("DA can't be decoded: {}", err);
                });
                //println!("da: {:?}", da);

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
                        prev_randao: block.mix_hash,
                        suggested_fee_recipient: meta.coinbase,
                        withdrawals: Some(vec![]),
                        parent_beacon_block_root: block.parent_beacon_block_root,
                    },
                    transactions: Some(filtered_transactions.clone()),
                    chain_da: chain_da.clone(),
                    gas_limit: None,
                };

                let l1_state_provider = self
                    .ctx
                    .provider()
                    .database_provider_ro()
                    .unwrap()
                    .state_provider_by_block_number(block.number)
                    .unwrap();

                let l1_block_number = block.number;

                let mut builder_attrs =
                    GwynethPayloadBuilderAttributes::try_new(B256::ZERO, attrs).unwrap();
                builder_attrs.providers.insert(self.ctx.config.chain.chain().id(), Arc::new(l1_state_provider));

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

                let payload_id = builder_attrs.inner.payload_id();
                let parrent_beacon_block_root =
                    builder_attrs.inner.parent_beacon_block_root.unwrap();

                //println!("payload_id: {} {}", node_idx, payload_id);

                // trigger new payload building draining the pool
                self.nodes[node_idx].payload_builder.new_payload(builder_attrs).await.unwrap();

                // wait for the payload builder to have finished building
                let mut payload =
                    EthBuiltPayload::new(payload_id, SealedBlock::default(), U256::ZERO);
                loop {
                    let result = self.nodes[node_idx].payload_builder.best_payload(payload_id).await;

                    if let Some(result) = result {
                        if let Ok(new_payload) = result {
                            payload = new_payload;
                            if payload.block().body.is_empty() {
                                tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
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

                //tokio::time::sleep(std::time::Duration::from_millis(10000)).await;

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


                // Determine the finalized block hash
                let finalized_hash: revm::primitives::FixedBytes<32> = self.get_finalized_hash(l1_block_number, node_idx);

                // println!("finalized hash: {}", finalized_hash);
                // println!("Block number: {}", payload.block().number);
                // println!("parent header: {}", payload.block().parent_hash);

                // let finalized_hash = GENESIS_HASH;

                // trigger forkchoice update via engine api to commit the block to the blockchain
                self.engine_apis[node_idx].update_forkchoice(finalized_hash, block_hash).await?;

                // loop {
                //     // wait for the block to commit
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
                //     println!("waiting on L2 block for {}: {}", node_chain_id, payload.block().number);
                //     tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                // }

                println!("[L1 block {}] Done with block {}: {}", block.number, node_chain_id, payload.block().number);

                let mut l2_block_indices = GWYNETH_SYNCED_L2_BLOCK_IDX.lock().unwrap();
                l2_block_indices.insert(self.nodes[node_idx].chain_spec().chain().id(), payload.block().number);


                // reorg stuff

                // Set l2_genesis_l1_block if this is the first L2 block
                if self.l2_genesis_l1_block == 0 {
                    self.l2_genesis_l1_block = l1_block_number;
                }

                // // Determine the finalized block hash
                // let finalized_hash = self.get_finalized_hash(l1_block_number, node_idx);
                // Determine the finalized block hash
                //let finalized_hash = self.get_finalized_hash(l1_block_number, node_idx);
                // Convert l2_block_number to u64 if necessary
                //let l2_block_u64 = payload.block().number;

                // println!("finalized hash: {}", finalized_hash);
                // println!("Block number: {}", payload.block().number);
                // println!("parent header: {}", payload.block().parent_hash);

                // let finalized_hash = GENESIS_HASH;

                // // Update the L1-L2 mapping in the ring buffer
                self.update_l1_l2_ring_buffer(l1_block_number, payload.block().number, block_hash, node_idx);
                // // trigger forkchoice update via engine api to commit the block to the blockchain
                // self.engine_apis[node_idx].update_forkchoice(finalized_hash, block_hash).await?;

                // // Check against the counter instead of l2_block_u64
                // if payload.block().number == 2 {
                //     println!("REVERT");
                //     //Check reverting back to state 1, which shall be proposed before block nr 65.. and we will go back to that.
                //     self.revert_test(85, node_idx).await?;
                // }

                self.num_l2_blocks += 1;
            }
        }

        Ok(())
    }

    pub async fn revert(&mut self, chain: &Chain, node_idx: usize) -> eyre::Result<()> {
        // Find the oldest L1 block number (and subtract 1) in the given chain
        let oldest_l1_block = chain.blocks().keys().min().copied()
            .ok_or_else(|| eyre::eyre!("Chain is empty"))?
            .saturating_sub(1);

        // Find the corresponding or closest prior L2 block
        let l2_block_hash = self.find_l2_block_hash(oldest_l1_block, node_idx);

        // Update forkchoice
        if let Some(block_hash) = l2_block_hash {
            self.engine_apis[node_idx].update_forkchoice(block_hash, block_hash).await?;
            // Remove all mappings newer than the reverted block
            self.l1_l2_ring_buffers[node_idx].retain(|mapping| mapping.l1_block <= oldest_l1_block);
        }

        // Remove all mappings newer than the reverted block
        self.l1_l2_ring_buffers[node_idx].retain(|mapping| mapping.l1_block <= oldest_l1_block);

        Ok(())
    }

    pub async fn revert_test(&mut self, oldest_l1_block: u64, node_idx: usize) -> eyre::Result<()> {
        // Find the corresponding or closest prior L2 block
        let l2_block_hash = self.find_l2_block_hash(oldest_l1_block, node_idx);

        //println!("Dani: l1_block we need a snapshot from {}", oldest_l1_block);
        //println!("Dani: l2_blockhash we found {:?}", l2_block_hash);

        // Update forkchoice
        //if let Some(block_hash) = l2_block_hash {

            //println!("Dani: reverting to: {}", block_hash);
            //self.engine_apis[node_idx].update_forkchoice(block_hash, block_hash).await?;
            let res = self.engine_apis[node_idx].update_forkchoice(GENESIS_HASH, GENESIS_HASH).await?;

            println!("Dani: reverted: {:?}", res);
             // Remove all mappings newer than the reverted block
            self.l1_l2_ring_buffers[node_idx].retain(|mapping| mapping.l1_block <= oldest_l1_block);
        //}

        Ok(())
    }

    fn find_l2_block_hash(&self, l1_block: u64, node_idx: usize) -> Option<B256> {
        println!("ring buffer: {:?}", self.l1_l2_ring_buffers[node_idx]);
        // Find the exact match or the closest prior L2 block
        self.l1_l2_ring_buffers[node_idx]
            .iter()
            .rev()
            .find(|mapping| mapping.l1_block <= l1_block)
            .map(|mapping| mapping.l2_hash)
    }

    fn update_l1_l2_ring_buffer(&mut self, l1_block: u64, l2_block: u64, l2_hash: B256, node_idx: usize) {
        // Check if we already have an L2 enthy for this L1 block
        if let Some(existing_index) = self.l1_l2_ring_buffers[node_idx].iter().position(|m| m.l1_block == l1_block) {
            // We have an existing entry, check if the new L2 block is higher
            let existing_mapping = &mut self.l1_l2_ring_buffers[node_idx][existing_index];
            if l2_block > existing_mapping.l2_block {
                existing_mapping.l2_block = l2_block;
                existing_mapping.l2_hash = l2_hash;
            }
        } else {
            // No existing entry for this L1 block, add a new one
            let mapping = L1L2Mapping {
                l1_block,
                l2_block,
                l2_hash,
            };

            if self.l1_l2_ring_buffers[node_idx].len() == RING_BUFFER_SIZE {
                // If the buffer is full, remove the oldest entry
                self.l1_l2_ring_buffers[node_idx].pop_front();
            }

            // Add the new mapping to the end of the buffer
            self.l1_l2_ring_buffers[node_idx].push_back(mapping);
        }
    }

    // New method to get the L2 block info for a given L1 block number
    pub fn get_l2_info_for_l1_block(&self, l1_block: u64, node_idx: usize) -> Option<(u64, B256)> {
        self.l1_l2_ring_buffers[node_idx]
            .iter()
            .find(|mapping| mapping.l1_block == l1_block)
            .map(|mapping| (mapping.l2_block, mapping.l2_hash))
    }

    fn get_finalized_hash(&self, current_l1_block: u64, node_idx: usize) -> B256 {
        if current_l1_block < self.l2_genesis_l1_block + FINALIZATION_PERIOD {
            return GENESIS_HASH;
        }

        // Calculate the highest block number in the previous finalization period
        // Example:
        // If we're at L1 block 65, this will give us 63
        // If we're at L1 block 128, this will give us 127
        let highest_finalized_l1_block = ((current_l1_block - 1) / FINALIZATION_PERIOD) * FINALIZATION_PERIOD - 1;

        // Find the L2 block hash for the highest finalized L1 block or the closest prior block
        for i in (self.l2_genesis_l1_block..=highest_finalized_l1_block).rev() {
            if let Some(mapping) = self.l1_l2_ring_buffers[node_idx]
                .iter()
                .rev()
                .find(|m| m.l1_block == i)
            {
                return mapping.l2_hash;
            }
        }

        // If no suitable block is found, return GENESIS_HASH
        GENESIS_HASH
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
