use crate::{
    AccountReader, BlockHashReader, ExecutionDataProvider, StateProvider, StateRootProvider,
};
use reth_primitives::{
    Account, Address, BlockNumber, Bytecode, Bytes, B256,
};
use reth_storage_api::{StateProofProvider, StorageRootProvider};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{
    prefix_set::TriePrefixSetsMut, updates::TrieUpdates, AccountProof, HashedPostState,
    HashedStorage,
};
use revm::{db::BundleAccount, primitives::ChainAddress};
use std::collections::HashMap;

/// A state provider that resolves to data from either a wrapped [`crate::ExecutionOutcome`]
/// or an underlying state provider.
///
/// This struct combines two sources of state data: the execution outcome and an underlying
/// state provider. It can provide state information by leveraging both the post-block execution
/// changes and the pre-existing state data.
#[derive(Debug)]
pub struct BundleStateProvider<SP: StateProvider, EDP: ExecutionDataProvider> {
    /// The inner state provider.
    pub state_provider: SP,
    /// Block execution data.
    pub block_execution_data_provider: EDP,
}

impl<SP: StateProvider, EDP: ExecutionDataProvider> BundleStateProvider<SP, EDP> {
    /// Create new bundle state provider
    pub const fn new(state_provider: SP, block_execution_data_provider: EDP) -> Self {
        Self { state_provider, block_execution_data_provider }
    }

    pub fn filter_bundle_state(&self) -> HashMap<ChainAddress, BundleAccount> {
        let chain_id = self.block_execution_data_provider.execution_outcome().chain_id;
        self.block_execution_data_provider.execution_outcome().current_state().state
    }
}

/* Implement StateProvider traits */

impl<SP: StateProvider, EDP: ExecutionDataProvider> BlockHashReader
    for BundleStateProvider<SP, EDP>
{
    fn block_hash(&self, block_number: BlockNumber) -> ProviderResult<Option<B256>> {
        let block_hash = self.block_execution_data_provider.block_hash(block_number);
        if block_hash.is_some() {
            return Ok(block_hash)
        }
        self.state_provider.block_hash(block_number)
    }

    fn canonical_hashes_range(
        &self,
        _start: BlockNumber,
        _end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        unimplemented!()
    }
}

impl<SP: StateProvider, EDP: ExecutionDataProvider> AccountReader for BundleStateProvider<SP, EDP> {
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        if let Some(account) =
            self.block_execution_data_provider.execution_outcome().account(&address)
        {
            Ok(account)
        } else {
            self.state_provider.basic_account(address)
        }
    }
}

impl<SP: StateProvider, EDP: ExecutionDataProvider> StateRootProvider
    for BundleStateProvider<SP, EDP>
{
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        let bundle_state = self.filter_bundle_state();
        let mut state = HashedPostState::from_bundle_state(&bundle_state);
        state.extend(hashed_state);
        self.state_provider.state_root(state)
    }

    fn state_root_from_nodes(
        &self,
        _nodes: TrieUpdates,
        _hashed_state: HashedPostState,
        _prefix_sets: TriePrefixSetsMut,
    ) -> ProviderResult<B256> {
        unimplemented!()
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        let bundle_state = self.filter_bundle_state();
        let mut state = HashedPostState::from_bundle_state(&bundle_state);
        state.extend(hashed_state);
        self.state_provider.state_root_with_updates(state)
    }

    fn state_root_from_nodes_with_updates(
        &self,
        nodes: TrieUpdates,
        hashed_state: HashedPostState,
        prefix_sets: TriePrefixSetsMut,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        let bundle_state = self.filter_bundle_state();
        let mut state = HashedPostState::from_bundle_state(&bundle_state);
        let mut state_prefix_sets = state.construct_prefix_sets();
        state.extend(hashed_state);
        state_prefix_sets.extend(prefix_sets);
        self.state_provider.state_root_from_nodes_with_updates(nodes, state, state_prefix_sets)
    }
}

impl<SP: StateProvider, EDP: ExecutionDataProvider> StorageRootProvider
    for BundleStateProvider<SP, EDP>
{
    fn storage_root(
        &self,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        let chain_id = self.block_execution_data_provider.execution_outcome().chain_id;
        let bundle_state = self.block_execution_data_provider.execution_outcome().current_state();
        let mut storage = bundle_state
            .account(&ChainAddress(chain_id, address))
            .map(|account| {
                HashedStorage::from_plain_storage(
                    account.status,
                    account.storage.iter().map(|(slot, value)| (slot, &value.present_value)),
                )
            })
            .unwrap_or_else(|| HashedStorage::new(false));
        storage.extend(&hashed_storage);
        self.state_provider.storage_root(address, storage)
    }
}

impl<SP: StateProvider, EDP: ExecutionDataProvider> StateProofProvider
    for BundleStateProvider<SP, EDP>
{
    fn proof(
        &self,
        hashed_state: HashedPostState,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        let bundle_state =
            self.block_execution_data_provider.execution_outcome().current_state();
        let mut state = HashedPostState::from_bundle_state(&bundle_state.state);
        state.extend(hashed_state);
        self.state_provider.proof(state, address, slots)
    }

    fn witness(
        &self,
        overlay: HashedPostState,
        target: HashedPostState,
    ) -> ProviderResult<HashMap<B256, Bytes>> {
        let bundle_state = self.filter_bundle_state();
        let mut state = HashedPostState::from_bundle_state(&bundle_state);
        state.extend(overlay);
        self.state_provider.witness(state, target)
    }
}

impl<SP: StateProvider, EDP: ExecutionDataProvider> StateProvider for BundleStateProvider<SP, EDP> {
    fn storage(
        &self,
        account: Address,
        storage_key: reth_primitives::StorageKey,
    ) -> ProviderResult<Option<reth_primitives::StorageValue>> {
        let u256_storage_key = storage_key.into();
        if let Some(value) = self
            .block_execution_data_provider
            .execution_outcome()
            .storage(&account, u256_storage_key)
        {
            return Ok(Some(value))
        }

        self.state_provider.storage(account, storage_key)
    }

    fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>> {
        if let Some(bytecode) =
            self.block_execution_data_provider.execution_outcome().bytecode(&code_hash)
        {
            return Ok(Some(bytecode))
        }

        self.state_provider.bytecode_by_hash(code_hash)
    }
}
