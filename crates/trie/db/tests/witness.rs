use alloy_rlp::EMPTY_STRING_CODE;
use reth_primitives::{constants::EMPTY_ROOT_HASH, keccak256, Account, Address, Bytes, B256, U256};
use reth_provider::{test_utils::create_test_provider_factory, HashingWriter};
use reth_trie::{proof::Proof, witness::TrieWitness, HashedPostState, HashedStorage, StateRoot};
use reth_trie_db::{DatabaseProof, DatabaseStateRoot, DatabaseTrieWitness};
use std::collections::{HashMap, HashSet};

#[test]
fn includes_empty_node_preimage() {
    let factory = create_test_provider_factory();
    let provider = factory.provider_rw().unwrap();

    // witness includes empty state trie root node
    assert_eq!(
        TrieWitness::from_tx(provider.tx_ref()).compute(HashedPostState::default()).unwrap(),
        HashMap::from([(EMPTY_ROOT_HASH, Bytes::from([EMPTY_STRING_CODE]))])
    );

    let address = Address::random();
    let hashed_address = keccak256(address);
    let hashed_slot = B256::random();

    // Insert account into database
    provider.insert_account_for_hashing([(address, Some(Account::default()))]).unwrap();

    let state_root = StateRoot::from_tx(provider.tx_ref()).root().unwrap();
    let multiproof = Proof::from_tx(provider.tx_ref())
        .with_target((hashed_address, HashSet::from([hashed_slot])))
        .multiproof()
        .unwrap();

    let witness = TrieWitness::from_tx(provider.tx_ref())
        .compute(HashedPostState {
            accounts: HashMap::from([(hashed_address, Some(Account::default()))]),
            storages: HashMap::from([(
                hashed_address,
                HashedStorage::from_iter(false, [(hashed_slot, U256::ZERO)]),
            )]),
        })
        .unwrap();
    assert!(witness.contains_key(&state_root));
    for node in multiproof.account_subtree.values() {
        assert_eq!(witness.get(&keccak256(&node)), Some(node));
    }
    // witness includes empty state trie root node
    assert_eq!(witness.get(&EMPTY_ROOT_HASH), Some(&Bytes::from([EMPTY_STRING_CODE])));
}
