use std::{collections::{HashMap, VecDeque}, marker::PhantomData, sync::Arc};
use alloy_consensus::{Blob, SidecarCoder, SimpleCoder, Transaction};
use alloy_rlp::Decodable;
use alloy_sol_types::{sol, SolEventInterface};
use alloy_eips::eip4844::kzg_to_versioned_hash;
use alloy_primitives::{address, b256};
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
    Address, Bytes, ChainDA, GwynethDA, SealedBlock, SealedBlockWithSenders, StateDiff, TransactionSigned, B256, U256
};
use reth_provider::{
    providers::BlockchainProvider, BlockNumReader, CanonStateSubscriptions, DatabaseProviderFactory,
};
use reth_rpc_types::{engine::PayloadStatusEnum, BlockNumberOrTag};
use reth_transaction_pool::{
    blobstore::DiskFileBlobStore, CoinbaseTipOrdering, EthPooledTransaction,
    EthTransactionValidator, Pool, TransactionValidationTaskExecutor, TransactionPool,
};
use RollupContract::{BlockProposed, RollupContractEvents};
use reth_provider::BlockReaderIdExt;
use reth_provider::{GWYNETH_SYNCED_L1_BLOCK_IDX, GWYNETH_SYNCED_L2_BLOCK_IDX};

const ROLLUP_CONTRACT_ADDRESS: Address = address!("9fCF7D13d10dEdF17d0f24C62f0cf4ED462f65b7");
pub const BASE_CHAIN_ID: u64 = 167010;
const FINALIZATION_PERIOD: u64 = 64;
const GENESIS_HASH: B256 = b256!("930c04603408ca90f730fb9ad792af0d42bd01db97633df8f9f58330cf103b0c");

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
    l1_to_l2: HashMap<u64, HashMap<u64, (u64, B256)>>,
    l1_finalized_block: u64,
}

impl<Node: reth_node_api::FullNodeComponents> Rollup<Node> {
    pub async fn new(ctx: ExExContext<Node>, nodes: Vec<GwynethFullNode>) -> eyre::Result<Self> {
        let mut engine_apis = Vec::new();
        let mut l2_block_info = HashMap::new();
        for node in &nodes {
            let engine_api = EngineApiContext {
                engine_api_client: node.auth_server_handle().http_client(),
                canonical_stream: node.provider.canonical_state_stream(),
                _marker: PhantomData::<GwynethEngineTypes>,
            };
            engine_apis.push(engine_api);

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
        while let Some(notification) = self.ctx.notifications.recv().await {
            if let Some(reverted_chain) = notification.reverted_chain() {
                // Find the oldest L1 block number (and subtract 1) in the given chain to get the newest latest L1 block
                let target_l1_block = reverted_chain.blocks().keys().min().copied()
                    .ok_or_else(|| eyre::eyre!("Chain is empty"))?
                    .saturating_sub(1);

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
                println!("EXEX called for block {}", committed_chain.tip().number);

                // Copy the previous l1 -> L2 mapping to the current block, which may get overwritten below with new blocks
                for block in committed_chain.blocks_iter() {
                    self.l1_to_l2.insert(block.number, self.l1_to_l2.get(&(block.number - 1)).unwrap().clone());
                }
                //self.l1_finalized_block = committed_chain.tip().number.saturating_sub(FINALIZATION_PERIOD);
                // prune list
                //self.l1_to_l2.retain(|&l1_block, _| l1_block >= self.l1_finalized_block);

                // Sync nodes
                for i in 0..self.nodes.len() {
                    self.commit(&committed_chain, i).await?;
                }

                // Update the sync data
                unsafe {
                    GWYNETH_SYNCED_L1_BLOCK_IDX = committed_chain.tip().number;
                    println!("Updated L1 sync data: {}", GWYNETH_SYNCED_L1_BLOCK_IDX);
                }

                self.ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().number))?;

                // if committed_chain.tip().number == 20 {
                //     println!("REVERT");
                //     for i in 0..self.nodes.len() {
                //         self.revert(i, 10).await?;
                //     }
                // }
            }
        }

        Ok(())
    }

    // TODO(Brecht): handle block by block instead of chain (easier to manage reorg stuff)
    pub async fn commit(&mut self, chain: &Chain, node_idx: usize) -> eyre::Result<()> {
        let events = decode_chain_into_rollup_events(chain);
        // println!("num events: {:?}", events.len());

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

        for (block, tx, event) in events {
            if let RollupContractEvents::BlockProposed(BlockProposed {
                block: ultra_block,
            }) = event
            {
                // Decode blobs and concatenate them to get the raw transactions
                let data = get_blob_data(self.ctx.pool(), tx, ultra_block.blobHashes)?;
                assert_eq!(data, ultra_block.da, "blob data does not match calldata");

                let (da, tx_list) = bincode::deserialize::<(GwynethDA, Vec<u8>)>(&ultra_block.da)
                    .unwrap_or_else(|err| {
                        panic!("DA can't be decoded: {}", err);
                    });

                let transactions: Vec<TransactionSigned> = decode_transactions(&tx_list);
                println!("transactions: {:?}", transactions.len());

                //println!("da: {:?}", da);

                let all_transactions: Vec<TransactionSigned> = decode_transactions(&tx_list);
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
                        suggested_fee_recipient: ultra_block.blocks[0].coinbase,
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

                // Determine the finalized block hash for this L2
                let finalized_hash = self.l1_to_l2.get(&self.l1_finalized_block).unwrap().get(&node_chain_id).unwrap().1;

                // println!("finalized hash: {}", finalized_hash);
                // println!("Block number: {}", payload.block().number);

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

fn get_blob_data<Pool: TransactionPool>(pool: &Pool, tx: &TransactionSigned, blob_hashes: Vec<B256>) -> eyre::Result<Vec<u8>> {
    let blobs: Vec<_> = if let Some(sidecar) = pool.get_blob(tx.hash())? {
        // Try to get blobs from the transaction pool
        sidecar.blobs.clone().into_iter().zip(sidecar.commitments.clone()).collect()
    } else {
        Vec::new()
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
