from web3 import Web3
from eth_abi import encode
import argparse

RPC_URL_L2 = 'http://127.0.0.1:' # Anything is fine for now as long as we dont have the L2 network, but if we have we can automate nonce and gas settings
w3_taiko_l2 = Web3(Web3.HTTPProvider(RPC_URL_L2)) 

# Some pre-loaded ETH addresses from Kurtosis private network (NO secret, no harm to use for private testnets!)
sender_addresses =  ['0x802dCbE1B1A97554B4F50DB5119E37E8e7336417']
sender_pks = ['5d2344259f42259f82d2c140aa66102ba89b57b4883ee441a8b312622bd42491']

receiver = '0xf93Ee4Cf8c6c40b329b0c0626F28333c132CF241' # This address also has pre-loaded ETH addresses

# parser = argparse.ArgumentParser()

# parser.add_argument("-p", "--port", help="port on localhost",
#                     type=str, required=True)
# parser.add_argument("-c", "--chainid", help="l2 chainId",
#                     type=int, required=True)

transaction_list = []

if __name__ == "__main__":
    # args = parser.parse_args()
    port = "32005"
    w3_taiko_l2 = Web3(Web3.HTTPProvider(RPC_URL_L2+port))
    chainId = 167010

    # Build the new tx list
    idx = 0
    for sender in sender_addresses:
        # Build the tx
        transaction = {
            'chainId': chainId,
            'from': sender,
            'to': receiver,
            'value': w3_taiko_l2.to_wei('250', 'ether'),
            'nonce': w3_taiko_l2.eth.get_transaction_count(sender),
            'gas': 200000,
            'maxFeePerGas': 2000000000, # w3_taiko_l2.eth.gas_price or something
            'maxPriorityFeePerGas': 1000000000,
        }

        # Debug prints of balance
        # # Get the balance
        # balance_wei = w3_taiko_l2.eth.get_balance(sender)

        # # Convert balance from Wei to Ether
        # balance_eth = w3_taiko_l2.from_wei(balance_wei, 'ether')
        # print("Balance before:", balance_eth)

        # 2. Sign tx with a private key
        signed_txn = w3_taiko_l2.eth.account.sign_transaction(transaction, sender_pks[idx])

        # print("RawTransaction:")
        # print(signed_txn.rawTransaction)
        print("RawTransaction.hex():")
        print(signed_txn.rawTransaction.hex())

        txn_hash = w3_taiko_l2.eth.send_raw_transaction(signed_txn.rawTransaction)
        print("Txn hash:")
        print(txn_hash.hex())

        # # Get the balance
        # balance_wei = w3_taiko_l2.eth.get_balance(sender)

        # # Convert balance from Wei to Ether
        # balance_eth = w3_taiko_l2.from_wei(balance_wei, 'ether')
        # print("Balance after:", balance_eth)