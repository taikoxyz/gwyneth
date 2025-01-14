//! A chain in a [`BlockchainTree`][super::BlockchainTree].
//!
//! A [`Chain`] contains the state of accounts for the chain after execution of its constituent
//! blocks, as well as a list of the blocks the chain is composed of.

use super::externals::TreeExternals;
use crate::BundleStateDataRef;
use reth_blockchain_tree_api::{
    error::{BlockchainTreeError, InsertBlockErrorKind},
    BlockAttachment, BlockValidationKind,
};
use reth_consensus::{Consensus, ConsensusError, PostExecutionInput};
use reth_db_api::database::Database;
use reth_evm::execute::{BlockExecutorProvider, Executor};
use reth_execution_errors::BlockExecutionError;
use reth_execution_types::{Chain, ExecutionOutcome};
use reth_primitives::{
    BlockHash, BlockNumber, ForkBlock, GotExpected, SealedBlockWithSenders, SealedHeader, B256, U256
};
use reth_provider::{
    providers::{BundleStateProvider, ConsistentDbView}, AccountReader, BlockNumReader, ChainSpecProvider, DatabaseProviderFactory, FullExecutionDataProvider, ProviderError, StateProvider, StateRootProvider, NODES
};
use reth_revm::database::{StateProviderDatabase, SyncStateProviderDatabase};
use reth_trie::{updates::TrieUpdates, HashedPostState};
use reth_trie_parallel::parallel_root::ParallelStateRoot;
use std::{
    collections::BTreeMap,
    ops::{Deref, DerefMut},
    time::Instant,
};
use reth_execution_types::state_diff_to_block_execution_output;

/// A chain in the blockchain tree that has functionality to execute blocks and append them to
/// itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppendableChain {
    chain: Chain,
}

impl Deref for AppendableChain {
    type Target = Chain;

    fn deref(&self) -> &Self::Target {
        &self.chain
    }
}

impl DerefMut for AppendableChain {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.chain
    }
}

impl AppendableChain {
    /// Create a new appendable chain from a given chain.
    pub const fn new(chain: Chain) -> Self {
        Self { chain }
    }

    /// Get the chain.
    pub fn into_inner(self) -> Chain {
        self.chain
    }

    /// Create a new chain that forks off of the canonical chain.
    ///
    /// if [`BlockValidationKind::Exhaustive`] is specified, the method will verify the state root
    /// of the block.
    pub fn new_canonical_fork<DB, E>(
        block: SealedBlockWithSenders,
        parent_header: &SealedHeader,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        canonical_fork: ForkBlock,
        externals: &TreeExternals<DB, E>,
        block_attachment: BlockAttachment,
        block_validation_kind: BlockValidationKind,
    ) -> Result<Self, InsertBlockErrorKind>
    where
        DB: Database + Clone,
        E: BlockExecutorProvider,
    {
        let execution_outcome = ExecutionOutcome::default();
        let empty = BTreeMap::new();

        let state_provider = BundleStateDataRef {
            execution_outcome: &execution_outcome,
            sidechain_block_hashes: &empty,
            canonical_block_hashes,
            canonical_fork,
        };

        println!("Brecht: new_canonical_fork");
        let (bundle_state, trie_updates) = Self::validate_and_execute(
            block.clone(),
            parent_header,
            state_provider,
            externals,
            block_attachment,
            block_validation_kind,
        )?;

        Ok(Self { chain: Chain::new(vec![block], bundle_state, trie_updates) })
    }

    /// Create a new chain that forks off of an existing sidechain.
    ///
    /// This differs from [`AppendableChain::new_canonical_fork`] in that this starts a new fork.
    pub(crate) fn new_chain_fork<DB, E>(
        &self,
        block: SealedBlockWithSenders,
        side_chain_block_hashes: BTreeMap<BlockNumber, BlockHash>,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        canonical_fork: ForkBlock,
        externals: &TreeExternals<DB, E>,
        block_validation_kind: BlockValidationKind,
    ) -> Result<Self, InsertBlockErrorKind>
    where
        DB: Database + Clone,
        E: BlockExecutorProvider,
    {
        println!("AppendableChain::new_chain_fork");
        let parent_number =
            block.number.checked_sub(1).ok_or(BlockchainTreeError::GenesisBlockHasNoParent)?;
        let parent = self.blocks().get(&parent_number).ok_or(
            BlockchainTreeError::BlockNumberNotFoundInChain { block_number: parent_number },
        )?;

        // Filter out the bundle state that belongs to the current chain.
        let mut execution_outcome = self.execution_outcome().filter_current_chain();

        // Revert state to the state after execution of the parent block
        execution_outcome.revert_to(parent.number);

        // Revert changesets to get the state of the parent that we need to apply the change.
        let bundle_state_data = BundleStateDataRef {
            execution_outcome: &execution_outcome,
            sidechain_block_hashes: &side_chain_block_hashes,
            canonical_block_hashes,
            canonical_fork,
        };
        println!("Brecht: new_chain_fork");
        let (block_state, _) = Self::validate_and_execute(
            block.clone(),
            parent,
            bundle_state_data,
            externals,
            BlockAttachment::HistoricalFork,
            block_validation_kind,
        )?;
        // extending will also optimize few things, mostly related to selfdestruct and wiping of
        // storage.
        execution_outcome.extend(block_state);

        // remove all receipts and reverts (except the last one), as they belong to the chain we
        // forked from and not the new chain we are creating.
        let size = execution_outcome.receipts().len();
        execution_outcome.receipts_mut().drain(0..size - 1);
        // take all states since we have filtered
        execution_outcome.all_states_mut().take_n_reverts(size - 1);
        execution_outcome.set_first_block(block.number);

        // If all is okay, return new chain back. Present chain is not modified.
        Ok(Self { chain: Chain::from_block(block, execution_outcome, None) })
    }

    /// Validate and execute the given block that _extends the canonical chain_, validating its
    /// state root after execution if possible and requested.
    ///
    /// Note: State root validation is limited to blocks that extend the canonical chain and is
    /// optional, see [`BlockValidationKind`]. So this function takes two parameters to determine
    /// if the state can and should be validated.
    ///   - [`BlockAttachment`] represents if the block extends the canonical chain, and thus we can
    ///     cache the trie state updates.
    ///   - [`BlockValidationKind`] determines if the state root __should__ be validated.
    // fn validate_and_execute<EDP, DB, E>(
    //     block: SealedBlockWithSenders,
    //     parent_block: &SealedHeader,
    //     bundle_state_data_provider: EDP,
    //     externals: &TreeExternals<DB, E>,
    //     block_attachment: BlockAttachment,
    //     block_validation_kind: BlockValidationKind,
    // ) -> Result<(ExecutionOutcome, Option<TrieUpdates>), BlockExecutionError>
    // where
    //     EDP: FullExecutionDataProvider,
    //     DB: Database + Clone,
    //     E: BlockExecutorProvider,
    // {
    //     // some checks are done before blocks comes here.
    //     externals.consensus.validate_header_against_parent(&block, parent_block)?;

    //     // get the state provider.
    //     let canonical_fork = bundle_state_data_provider.canonical_fork();

    //     // SAFETY: For block execution and parallel state root computation below we open multiple
    //     // independent database transactions. Upon opening the database transaction the consistent
    //     // view will check a current tip in the database and throw an error if it doesn't match
    //     // the one recorded during initialization.
    //     // It is safe to use consistent view without any special error handling as long as
    //     // we guarantee that plain state cannot change during processing of new payload.
    //     // The usage has to be re-evaluated if that was ever to change.
    //     let consistent_view =
    //         ConsistentDbView::new_with_latest_tip(externals.provider_factory.clone())?;
    //     let state_provider = consistent_view
    //         .provider_ro()?
    //         // State root calculation can take a while, and we're sure no write transaction
    //         // will be open in parallel. See https://github.com/paradigmxyz/reth/issues/7509.
    //         .disable_long_read_transaction_safety()
    //         .state_provider_by_block_number(canonical_fork.number)?;


    //     // // Brecht
    //     // let mut super_db = SyncStateProviderDatabase::new(
    //     //     Some(externals.provider_factory.chain_spec().chain.id()),
    //     //     StateProviderDatabase::new(&provider),
    //     // );

    //     // // Add all external state dependencies
    //     // let providers = NODES.lock().unwrap();
    //     // let mut chain_providers = Vec::new();
    //     // for (&chain_id, chain_provider) in providers.iter() {
    //     //     println!("Adding db for chain_id: {}", chain_id);

    //     //     let state_provider = chain_provider
    //     //         .database_provider_ro()
    //     //         .unwrap();
    //     //     let last_block_number = state_provider.last_block_number()?;
    //     //     let state_provider = state_provider.state_provider_by_block_number(last_block_number).unwrap();

    //     //     let chain_provider = BundleStateProvider::new(state_provider, bundle_state_data_provider);
    //     //     chain_providers.push(chain_provider);

    //     //     //let boxed: Box<dyn StateProvider> = Box::new(state_provider);
    //     //     //let state_provider = StateProviderDatabase::new(boxed);
    //     //     super_db.add_db(chain_id, StateProviderDatabase::new(&chain_providers.last().unwrap()));
    //     // }

    //     // Brecht
    //     // let mut super_db: SyncStateProviderDatabase<Box<dyn StateProvider>> = SyncStateProviderDatabase::new(
    //     //     Some(externals.provider_factory.chain_spec().chain.id()),
    //     //     StateProviderDatabase::new(state_provider),
    //     // );

    //     // println!("Brecht executed: Adding db for chain_id: {}", externals.provider_factory.chain_spec().chain.id());
    //     // // Add all external state dependencies
    //     // let providers = NODES.lock().unwrap();
    //     // //let mut chain_providers = Vec::new();
    //     // for (&chain_id, chain_provider) in providers.iter() {
    //     //     if chain_id == externals.provider_factory.chain_spec().chain.id() {
    //     //         continue;
    //     //     }
    //     //     println!("Brecht executed: Adding db for chain_id: {}", chain_id);

    //     //     let state_provider = chain_provider
    //     //         .database_provider_ro()
    //     //         .unwrap();
    //     //     //let last_block_number = state_provider.last_block_number()?;
    //     //     let last_block_number = block.number - 1;
    //     //     println!("[{}-{}] Executing against block {} {}", externals.provider_factory.chain_spec().chain.id(), block.number, chain_id, last_block_number);
    //     //     let state_provider = state_provider.state_provider_by_block_number(last_block_number).unwrap();

    //     //     //let chain_provider = BundleStateProvider::new(state_provider, bundle_state_data_provider);
    //     //     //chain_providers.push(chain_provider);

    //     //     //let boxed: Box<dyn StateProvider> = Box::new(state_provider);
    //     //     //let state_provider = StateProviderDatabase::new(boxed);
    //     //     super_db.add_db(chain_id, StateProviderDatabase::new(state_provider));
    //     // }

    //     let block_number = block.number;
    //     let block_hash = block.hash();

    //     //println!("Block extra data: {:?}", block.extra_data);
    //     let state_diff = if block.extra_data.len() > 32 {
    //         //let json_str = String::from_utf8(block.extra_data.to_vec()).unwrap();
    //         //let state_diff = serde_json::from_str(&json_str).unwrap_or(None);
    //         //let state_diff: Option<reth_primitives::StateDiff> = serde_json::from_str(&json_str).unwrap_or(None);
    //         Some(bincode::deserialize(&block.extra_data.to_vec()).unwrap())
    //     } else {
    //         None
    //     };

    //     println!("Applying block update: {:?}", state_diff);

    //     let chain_id = externals.provider_factory.chain_spec().chain.id();

    //     let initial_execution_outcome = if let Some(state_diff) = &state_diff {
    //         println!("[{}] Applying state from diff", chain_id);

    //         // let block = block.clone().unseal();
    //         // let executor = externals.executor_factory.executor(super_db);
    //         // let state_execution = executor.execute((&block, U256::MAX).into())?;

    //         let mut state_execution_diff = state_diff_to_block_execution_output(chain_id, state_diff);
    //         state_execution_diff.state = state_diff.bundle.clone();
    //         state_execution_diff.receipts = state_diff.receipts.clone();

    //         for (tx, receipt) in block.transactions().zip(state_diff.receipts.iter()) {
    //             println!("{} [BBB] tx: {}: {:?}", chain_id, tx.hash(), receipt);
    //         }

    //         //println!("ori: {:?}", state_execution);
    //         //println!("new: {:?}", state_execution_diff);

    //         //let execution_outcome_execution = ExecutionOutcome::from((state_execution.clone(), chain_id, block_number)).filter_chain(chain_id);
    //         let execution_outcome_diff = ExecutionOutcome::from((state_execution_diff, chain_id, block_number));


    //         //println!("ori: {:?}", execution_outcome_execution.bundle);
    //         println!("new: {:?}", execution_outcome_diff.bundle);

    //         // for (address, account) in execution_outcome_diff.bundle.state.iter_mut() {
    //         //     //let provider = consistent_view.provider_ro().unwrap();
    //         //     //let account = provider.basic_account(address)
    //         //     //account.original_info = execution_outcome_execution.bundle.state.get(address).unwrap().original_info.clone();
    //         //     *account = execution_outcome_execution.bundle.state.get(address).unwrap().clone();
    //         // }


    //         println!("new: {:?}", execution_outcome_diff.bundle);

    //         // execution_outcome_diff.bundle = execution_outcome_execution.bundle.clone();

    //         // println!("ori: {:?}", execution_outcome_execution.bundle);
    //         // println!("new: {:?}", execution_outcome_diff.bundle);

    //         //execution_outcome_diff.bundle.state = execution_outcome_execution.bundle.state.clone();
    //         //execution_outcome_diff.bundle.contracts = execution_outcome_execution.bundle.contracts.clone();

    //         // execution_outcome_diff.bundle.state_size = execution_outcome_execution.bundle.state_size;
    //         // execution_outcome_diff.bundle.reverts_size = execution_outcome_execution.bundle.reverts_size;
    //         // execution_outcome_diff.bundle.reverts = execution_outcome_execution.bundle.reverts.clone();

    //         println!("new: {:?}", execution_outcome_diff);

    //         //let hashed_state_execution = execution_outcome_execution.hash_state_slow();
    //         let hashed_state_diff = execution_outcome_diff.hash_state_slow();
    //         //assert_eq!(hashed_state_execution, hashed_state_diff, "state diff incorrect");

    //         //println!("ori: {:?}", hashed_state_execution);
    //         println!("new: {:?}", hashed_state_diff);

    //         //execution_outcome_diff.bundle = state_diff.bundle.clone();

    //         execution_outcome_diff
    //     } else {
    //         let mut super_db: SyncStateProviderDatabase<Box<dyn StateProvider>> = SyncStateProviderDatabase::new(
    //             Some(externals.provider_factory.chain_spec().chain.id()),
    //             StateProviderDatabase::new(state_provider),
    //         );
    //         println!("Applying state from transactions");
    //         let block = block.clone().unseal();
    //         let executor = externals.executor_factory.executor(super_db);
    //         let state = executor.execute((&block, U256::MAX).into())?;

    //         // externals.consensus.validate_block_post_execution(
    //         //     &block,
    //         //     PostExecutionInput::new(&state.receipts, &state.requests),
    //         // )?;

    //         let initial_execution_outcome = ExecutionOutcome::from((state, chain_id, block_number));
    //         let initial_execution_outcome = initial_execution_outcome.filter_chain(chain_id);

    //         initial_execution_outcome
    //     };

    //     // SAFETY: For block execution and parallel state root computation below we open multiple
    //     // independent database transactions. Upon opening the database transaction the consistent
    //     // view will check a current tip in the database and throw an error if it doesn't match
    //     // the one recorded during initialization.
    //     // It is safe to use consistent view without any special error handling as long as
    //     // we guarantee that plain state cannot change during processing of new payload.
    //     // The usage has to be re-evaluated if that was ever to change.
    //     let consistent_view =
    //         ConsistentDbView::new_with_latest_tip(externals.provider_factory.clone())?;
    //     let state_provider = consistent_view
    //         .provider_ro()?
    //         // State root calculation can take a while, and we're sure no write transaction
    //         // will be open in parallel. See https://github.com/paradigmxyz/reth/issues/7509.
    //         .disable_long_read_transaction_safety()
    //         .state_provider_by_block_number(canonical_fork.number)?;

    //     let provider = BundleStateProvider::new(state_provider, bundle_state_data_provider);


    //     // check state root if the block extends the canonical chain __and__ if state root
    //     // validation was requested.
    //     if block_validation_kind.is_exhaustive() {
    //         // calculate and check state root
    //         let start = Instant::now();
    //         let (state_root, trie_updates) = if block_attachment.is_canonical() {
    //             println!("canonical block");
    //             // TODO(Cecilie): refactor the bundle state provider for cross-chain bundles
    //             //let mut execution_outcome =
    //             //    provider.block_execution_data_provider.execution_outcome().clone();
    //             //execution_outcome.chain_id = chain_id;
    //             //execution_outcome.extend(initial_execution_outcome.clone());
    //             //let execution_outcome = initial_execution_outcome.clone();

    //             let mut execution_outcome =
    //                 provider.block_execution_data_provider.execution_outcome().clone();
    //             execution_outcome.extend(initial_execution_outcome.clone());
    //             let hashed_state = execution_outcome.hash_state_slow();
    //             ParallelStateRoot::new(consistent_view, hashed_state)
    //                 .incremental_root_with_updates()
    //                 .map(|(root, updates)| (root, Some(updates)))
    //                 .map_err(ProviderError::from)?

    //             // let hashed_state = execution_outcome.hash_state_slow();
    //             // ParallelStateRoot::new(consistent_view, hashed_state)
    //             //     .incremental_root_with_updates()
    //             //     .map(|(root, updates)| (root, Some(updates)))
    //             //     .map_err(ProviderError::from)?
    //         } else {
    //             let hashed_state = HashedPostState::from_bundle_state(
    //                 &initial_execution_outcome.current_state().state,
    //             );
    //             let state_root = provider.state_root(hashed_state)?;
    //             //let state_root = super_db.get(&externals.provider_factory.chain_spec().chain.id()).unwrap().state_root(hashed_state)?;
    //             (state_root, None)
    //             //println!("Skipping check for chain {}", chain_id);
    //             //(B256::ZERO, None)
    //         };
    //         if block.state_root != state_root {
    //             return Err(ConsensusError::BodyStateRootDiff(
    //                 GotExpected { got: state_root, expected: block.state_root }.into(),
    //             )
    //             .into())
    //         }

    //         tracing::debug!(
    //             target: "blockchain_tree::chain",
    //             number = block.number,
    //             hash = %block_hash,
    //             elapsed = ?start.elapsed(),
    //             "Validated state root"
    //         );

    //         Ok((initial_execution_outcome, trie_updates))
    //     } else {
    //         Ok((initial_execution_outcome, None))
    //     }
    // }

    fn validate_and_execute<EDP, DB, E>(
        block: SealedBlockWithSenders,
        parent_block: &SealedHeader,
        bundle_state_data_provider: EDP,
        externals: &TreeExternals<DB, E>,
        block_attachment: BlockAttachment,
        block_validation_kind: BlockValidationKind,
    ) -> Result<(ExecutionOutcome, Option<TrieUpdates>), BlockExecutionError>
    where
        EDP: FullExecutionDataProvider,
        DB: Database + Clone,
        E: BlockExecutorProvider,
    {
        // some checks are done before blocks comes here.
        externals.consensus.validate_header_against_parent(&block, parent_block)?;

        // get the state provider.
        let canonical_fork = bundle_state_data_provider.canonical_fork();

        // SAFETY: For block execution and parallel state root computation below we open multiple
        // independent database transactions. Upon opening the database transaction the consistent
        // view will check a current tip in the database and throw an error if it doesn't match
        // the one recorded during initialization.
        // It is safe to use consistent view without any special error handling as long as
        // we guarantee that plain state cannot change during processing of new payload.
        // The usage has to be re-evaluated if that was ever to change.
        let consistent_view =
            ConsistentDbView::new_with_latest_tip(externals.provider_factory.clone())?;
        let state_provider = consistent_view
            .provider_ro()?
            // State root calculation can take a while, and we're sure no write transaction
            // will be open in parallel. See https://github.com/paradigmxyz/reth/issues/7509.
            .disable_long_read_transaction_safety()
            .state_provider_by_block_number(canonical_fork.number)?;

        let provider = BundleStateProvider::new(state_provider, bundle_state_data_provider);
        let chain_id = externals.provider_factory.chain_spec().chain.id();

        let db = SyncStateProviderDatabase::new(
            Some(externals.provider_factory.chain_spec().chain.id()),
            StateProviderDatabase::new(&provider),
        );

        let state_diff = if block.extra_data.len() > 32 {
            //let json_str = String::from_utf8(block.extra_data.to_vec()).unwrap();
            //let state_diff = serde_json::from_str(&json_str).unwrap_or(None);
            //let state_diff: Option<reth_primitives::StateDiff> = serde_json::from_str(&json_str).unwrap_or(None);
            Some(bincode::deserialize(&block.extra_data.to_vec()).unwrap())
        } else {
            None
        };
        let initial_execution_outcome = if let Some(state_diff) = &state_diff {
            println!("[{}] Applying state from diff", chain_id);

            // let block = block.clone().unseal();
            // let executor = externals.executor_factory.executor(super_db);
            // let state_execution = executor.execute((&block, U256::MAX).into())?;

            let state_execution_diff = state_diff_to_block_execution_output(chain_id, state_diff);
            //state_execution_diff.state = state_diff.bundle.clone();
            //state_execution_diff.receipts = state_diff.receipts.clone();

            for (tx, receipt) in block.transactions().zip(state_diff.receipts.iter()) {
                println!("{} [Applying] tx: {}: {:?}", chain_id, tx.hash(), receipt);
            }

            //println!("ori: {:?}", state_execution);
            //println!("new: {:?}", state_execution_diff);

            //let execution_outcome_execution = ExecutionOutcome::from((state_execution.clone(), chain_id, block_number)).filter_chain(chain_id);
            let execution_outcome_diff = ExecutionOutcome::from((state_execution_diff, chain_id, block.number));


            //println!("ori: {:?}", execution_outcome_execution.bundle);
            //println!("new: {:?}", execution_outcome_diff.bundle);

            // for (address, account) in execution_outcome_diff.bundle.state.iter_mut() {
            //     //let provider = consistent_view.provider_ro().unwrap();
            //     //let account = provider.basic_account(address)
            //     //account.original_info = execution_outcome_execution.bundle.state.get(address).unwrap().original_info.clone();
            //     *account = execution_outcome_execution.bundle.state.get(address).unwrap().clone();
            // }


            //println!("new: {:?}", execution_outcome_diff.bundle);

            // execution_outcome_diff.bundle = execution_outcome_execution.bundle.clone();

            // println!("ori: {:?}", execution_outcome_execution.bundle);
            // println!("new: {:?}", execution_outcome_diff.bundle);

            //execution_outcome_diff.bundle.state = execution_outcome_execution.bundle.state.clone();
            //execution_outcome_diff.bundle.contracts = execution_outcome_execution.bundle.contracts.clone();

            // execution_outcome_diff.bundle.state_size = execution_outcome_execution.bundle.state_size;
            // execution_outcome_diff.bundle.reverts_size = execution_outcome_execution.bundle.reverts_size;
            // execution_outcome_diff.bundle.reverts = execution_outcome_execution.bundle.reverts.clone();

            // println!("new: {:?}", execution_outcome_diff);

            //let hashed_state_execution = execution_outcome_execution.hash_state_slow();
            //let hashed_state_diff = execution_outcome_diff.hash_state_slow();
            //assert_eq!(hashed_state_execution, hashed_state_diff, "state diff incorrect");

            //println!("ori: {:?}", hashed_state_execution);
            //println!("new: {:?}", hashed_state_diff);

            //execution_outcome_diff.bundle = state_diff.bundle.clone();

            execution_outcome_diff
        } else {
            let executor = externals.executor_factory.executor(db);
            let block = block.clone().unseal();

            let state = executor.execute((&block, U256::MAX).into())?;
            externals.consensus.validate_block_post_execution(
                &block,
                PostExecutionInput::new(&state.receipts, &state.requests),
            )?;

            let chain_id = externals.provider_factory.chain_spec().chain.id();
            ExecutionOutcome::from((state, chain_id, block.number))
        };

        // check state root if the block extends the canonical chain __and__ if state root
        // validation was requested.
        if block_validation_kind.is_exhaustive() {
            // calculate and check state root
            let start = Instant::now();
            let (state_root, trie_updates) = if block_attachment.is_canonical() {
                let mut execution_outcome =
                    provider.block_execution_data_provider.execution_outcome().clone();
                execution_outcome.chain_id = chain_id;
                execution_outcome.extend(initial_execution_outcome.clone());
                let hashed_state = execution_outcome.hash_state_slow();
                ParallelStateRoot::new(consistent_view, hashed_state)
                    .incremental_root_with_updates()
                    .map(|(root, updates)| (root, Some(updates)))
                    .map_err(ProviderError::from)?
            } else {
                let hashed_state = HashedPostState::from_bundle_state(
                    &initial_execution_outcome.state(chain_id).state,
                );
                let state_root = provider.state_root(hashed_state)?;
                (state_root, None)
            };
            assert_eq!(block.state_root, state_root, "state root mismatch");
            if block.state_root != state_root {
                return Err(ConsensusError::BodyStateRootDiff(
                    GotExpected { got: state_root, expected: block.state_root }.into(),
                )
                .into())
            }

            tracing::debug!(
                target: "blockchain_tree::chain",
                number = block.number,
                hash = %block.hash(),
                elapsed = ?start.elapsed(),
                "Validated state root"
            );

            Ok((initial_execution_outcome, trie_updates))
        } else {
            Ok((initial_execution_outcome, None))
        }
    }

    /// Validate and execute the given block, and append it to this chain.
    ///
    /// This expects that the block's ancestors can be traced back to the `canonical_fork` (the
    /// first parent block of the `block`'s chain that is in the canonical chain).
    ///
    /// In other words, expects a gap less (side-) chain:  [`canonical_fork..block`] in order to be
    /// able to __execute__ the block.
    ///
    /// CAUTION: This will only perform state root check if it's possible: if the `canonical_fork`
    /// is the canonical head, or: state root check can't be performed if the given canonical is
    /// __not__ the canonical head.
    #[track_caller]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn append_block<DB, E>(
        &mut self,
        block: SealedBlockWithSenders,
        side_chain_block_hashes: BTreeMap<BlockNumber, BlockHash>,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        externals: &TreeExternals<DB, E>,
        canonical_fork: ForkBlock,
        block_attachment: BlockAttachment,
        block_validation_kind: BlockValidationKind,
    ) -> Result<(), InsertBlockErrorKind>
    where
        DB: Database + Clone,
        E: BlockExecutorProvider,
    {
        let parent_block = self.chain.tip();

        let bundle_state_data = BundleStateDataRef {
            execution_outcome: self.execution_outcome(),
            sidechain_block_hashes: &side_chain_block_hashes,
            canonical_block_hashes,
            canonical_fork,
        };

        println!("Brecht: append_block");
        let (block_state, _) = Self::validate_and_execute(
            block.clone(),
            parent_block,
            bundle_state_data,
            externals,
            block_attachment,
            block_validation_kind,
        )?;
        // extend the state.
        self.chain.append_block(block, block_state);

        Ok(())
    }
}
