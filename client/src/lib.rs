#![cfg_attr(not(feature = "std"), no_std)]

use core::{convert::TryInto, fmt};
use frame_support::{
	debug, decl_error, decl_event, decl_module, decl_storage, dispatch::DispatchResult,
};
use parity_scale_codec::{Encode, Decode};

use frame_system::{Origin, ensure_none, ensure_signed, offchain::{
		AppCrypto, CreateSignedTransaction, SendSignedTransaction, SendUnsignedTransaction,
		SignedPayload, Signer, SigningTypes, SubmitTransaction,
	}};
use sp_core::{crypto::KeyTypeId};
use sp_io::offchain_index;
use sp_runtime::{generic::UncheckedExtrinsic, offchain::http::Request};
use sp_runtime::{generic,
	offchain as rt_offchain,
	offchain::{
		storage::StorageValueRef,
		storage_lock::{BlockAndTime, StorageLock},
	},
	transaction_validity::{
		InvalidTransaction, TransactionSource, TransactionValidity, ValidTransaction,
	},
	RuntimeDebug,
};
use sp_runtime::traits::{BlakeTwo256, Block};
use sp_std::{collections::vec_deque::VecDeque, prelude::*, str};

use serde::{
	ser::{SerializeStruct, Serializer},
	Deserialize, Deserializer, Serialize
};
#[macro_use]
extern crate alloc;
// use hex_slice::AsHex;

// use sc_client_api::client;

pub type BlockNumber = u32;
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;

/// When an offchain worker is signing transactions it's going to request keys from type
/// `KeyTypeId` via the keystore to sign the transaction.
/// The keys can be inserted manually via RPC (see `author_insertKey`).
pub const KEY_TYPE: KeyTypeId = KeyTypeId(*b"demo");
// const NUM_VEC_LEN: usize = 10;
/// The type to sign and send transactions.
const UNSIGNED_TXS_PRIORITY: u64 = 100;

// TODO: api request
const HTTP_REMOTE_REQUEST: &str = "https://api.pro.coinbase.com/products/ETH-USD/ticker";

const HTTP_HEADER_USER_AGENT: &str = "jaminu71@gmail.com";

const HTTP_ETHEREUM_HOST: &str = "http://127.0.0.1:8545";

const BLOCK_FROM_ACCOUNT: &str = "0x7e4dC815bd24eC3741B01471FfEfF474cd0E0aB3";

const BLOCK_TO_ACCOUNT: &str = "0x85B72f750d1A22eD071e320a7Ce5fEbaA58B381d";

const FETCH_TIMEOUT_PERIOD: u64 = 3000; // in milli-seconds
const LOCK_TIMEOUT_EXPIRATION: u64 = FETCH_TIMEOUT_PERIOD + 1000; // in milli-seconds
const LOCK_BLOCK_EXPIRATION: u32 = 3; // in block number

const ONCHAIN_TX_KEY: &[u8] = b"ocw-demo::storage::tx";

/// Based on the above `KeyTypeId` we need to generate a pallet-specific crypto type wrapper.
/// We can utilize the supported crypto kinds (`sr25519`, `ed25519` and `ecdsa`) and augment
/// them with the pallet-specific identifier.
pub mod crypto {
	use crate::KEY_TYPE;
	use sp_core::sr25519::Signature as Sr25519Signature;
	use sp_runtime::app_crypto::{app_crypto, sr25519};
	use sp_runtime::{traits::Verify, MultiSignature, MultiSigner};

	app_crypto!(sr25519, KEY_TYPE);

	pub struct SupraAuthId;
	// implemented for ocw-runtime
	impl frame_system::offchain::AppCrypto<MultiSigner, MultiSignature> for SupraAuthId {
		type RuntimeAppPublic = Public;
		type GenericSignature = sp_core::sr25519::Signature;
		type GenericPublic = sp_core::sr25519::Public;
	}

	// implemented for mock runtime in test
	impl frame_system::offchain::AppCrypto<<Sr25519Signature as Verify>::Signer, Sr25519Signature>
		for SupraAuthId
	{
		type RuntimeAppPublic = Public;
		type GenericSignature = sp_core::sr25519::Signature;
		type GenericPublic = sp_core::sr25519::Public;
	}
}

#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
pub struct Payload<Public> {
	number: u64,
	public: Public,
}

impl<T: SigningTypes> SignedPayload<T> for Payload<T::Public> {
	fn public(&self) -> T::Public {
		self.public.clone()
	}
}

#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
pub struct EthPayLoad {
	eth_host: Vec<u8>,
	from_address: Vec<u8>,
	to_address: Vec<u8>,
}

/// This is the pallet's configuration trait
pub trait Config: frame_system::Config + CreateSignedTransaction<Call<Self>> {
    /// The identifier type for an offchain worker.
	type AuthorityId: AppCrypto<Self::Public, Self::Signature>;
	/// The overarching dispatch call type.
	type Call: From<Call<Self>>;
	/// The overarching event type.
	type Event: From<Event<Self>> + Into<<Self as frame_system::Config>::Event>;
}

decl_storage! {
    trait Store for Module<T: Config> as OcwDemo {
        /// A vector of recently submitted numbers. Bounded by NUM_VEC_LEN
		Numbers get(fn numbers): VecDeque<u64>;
	}
}

decl_event!(
    /// Events generated by the module.
	pub enum Event<T>
	where
    AccountId = <T as frame_system::Config>::AccountId,
	{
        /// Event generated when a new number is accepted to contribute to the average.
		NewNumber(Option<AccountId>, u64),
		/// Event fetching etherium price done and transaction made to ethereum.
		UpdateEthereumPrice(Option<AccountId>),
		/// Event submit etherium price done.
		SubmitEthereumPrice(Option<AccountId>, Vec<u8>),
		// GetCurrentPrice(Option<AccountId>, String),
	}
);

decl_error! {
    pub enum Error for Module<T: Config> {
        // Error returned when not sure which ocw function to executed
		UnknownOffchainMux,
        
		// Error returned when making signed transactions in off-chain worker
		NoLocalAcctForSigning,
		OffchainSignedTxError,
        
		// Error returned when making unsigned transactions in off-chain worker
		OffchainUnsignedTxError,
        
		// Error returned when making unsigned transactions with signed payloads in off-chain worker
		OffchainUnsignedTxSignedPayloadError,
        
		// Error returned when fetching github info
		HttpFetchingError,

		// Error if not parsed in given struct
		HttpNotParsedInStruct,

		// ParseFloatError
		ParseFloatError,

		// Get Parameter Error
		GetParamError
	}
}

decl_module! {
    pub struct Module<T: Config> for enum Call where origin: T::Origin {
        fn deposit_event() = default;


		#[weight = 10000]
		pub fn submit_ethereum_price(origin,ethereum_price: Vec<u8>) -> DispatchResult{

			let who = ensure_none(origin)?;

			let price_in_str = str::from_utf8(&ethereum_price).unwrap_or("error");
			
			debug::info!("updated ethereum price: ({:?}, {:?})", price_in_str, who);
			// debug::info!("inserted data {:?}", ethereum_price.to_vec());

			let key = Self::derived_key(frame_system::Module::<T>::block_number());
			let data = IndexingPriceData(b"submit_ethereum_price".to_vec(), ethereum_price.to_vec());
			offchain_index::set(&key, &data.encode());

			Self::deposit_event(RawEvent::SubmitEthereumPrice(None,ethereum_price));
			Ok(())
		}

		#[weight = 10000]
		pub fn update_ethereum_price(origin) -> DispatchResult{

			let who = ensure_signed(origin)?;
			
			debug::info!("Block Number: {:?}",frame_system::Module::<T>::block_number());

			let key = Self::derived_key(frame_system::Module::<T>::block_number());
			// let EthPayLoad { eth_host, from_address, to_address } = eth_payload;
			let data = IndexingPriceFlag(b"update_ethereum_price".to_vec());
			offchain_index::set(&key, &data.encode());

			Self::deposit_event(RawEvent::UpdateEthereumPrice(Some(who)));
			Ok(())
		}
        
		fn offchain_worker(block_number: T::BlockNumber) {
            debug::info!("Entering off-chain worker");
			debug::info!("off chain block number: {:?}", block_number);

			let key = Self::derived_key(block_number);
			let oci_mem = StorageValueRef::persistent(&key);
            
			// if let Some(Some(data)) = oci_mem.get::<IndexingData>() {
            //     debug::info!("off-chain indexing data: {:?}, {:?}",
            //     str::from_utf8(&data.0).unwrap_or("error"), data.1);
			// } else {
            //     debug::info!("no off-chain indexing data retrieved.");
			// }

			if let Some(Some(edata)) = oci_mem.get::<IndexingPriceFlag>() {
				let tran_name = str::from_utf8(&edata.0).unwrap_or("error");
				if tran_name == "update_ethereum_price" {
					let eth_host = HTTP_ETHEREUM_HOST;
					let from_address = BLOCK_FROM_ACCOUNT;
					let to_address = BLOCK_TO_ACCOUNT;
					debug::info!("HOST: {}, FROM: {}, TO: {}", &eth_host, &from_address, &to_address);
					let result = Self::update_ethereum_price_worker(eth_host, from_address, to_address);
					if let Err(e) = result {
						debug::error!("offchain_worker ethereum error: {:?}", e);
					}
				}
                debug::info!("off-chain ethereum indexing data: {:?}",
                tran_name);
			} else {
                debug::info!("no off-chain ethereum indexing data retrieved.");
			}
		}
	}
}

impl<T: Config> Module<T> {    
	pub fn derived_key(block_number: T::BlockNumber) -> Vec<u8> {
        block_number.using_encoded(|encoded_bn| {
            ONCHAIN_TX_KEY.clone().into_iter()
            .chain(b"/".into_iter())
            .chain(encoded_bn)
            .copied()
            .collect::<Vec<u8>>()
		})
	}

	pub fn update_ethereum_price_worker(eth_host: &str, from_address: &str, to_address: &str) -> Result<(), Error<T>> {

		debug::info!("Called Update Ethereum price worker");
		// let signer = Signer::<T, T::AuthorityId>::any_account();
        
		// let number: u64 = block_number.try_into().unwrap_or(0);

		let s_info = StorageValueRef::persistent(b"ocw-demo::ethereum-info");

		let mut lock = StorageLock::<BlockAndTime<Self>>::with_block_and_time_deadline(
            b"ocw-demo::lock",
			LOCK_BLOCK_EXPIRATION,
			rt_offchain::Duration::from_millis(LOCK_TIMEOUT_EXPIRATION),
		);

		if let Ok(_guard) = lock.try_lock() {
			match Self::fetch_n_parse() {
				Ok(data) => {
                    s_info.set(&data);
				}
				Err(err) => {
                    return Err(err);
				}
			}
		}
		
		let final_price:Vec<u8>;
		if let Some(Some(ethereum_price_data)) = s_info.get::<LightClient>() {
			final_price = ethereum_price_data.price;	
		} else {
			final_price = "0".into();
		}

		let _result = Self::submt_price_to_ethereum(final_price.clone(), eth_host, from_address, to_address)?;

		let resp_str = str::from_utf8(&_result).map_err(|_| <Error<T>>::HttpFetchingError)?;

		debug::info!("Response from ethereum: {:?}",resp_str);

		// Ok(())

		let call = Call::submit_ethereum_price(final_price.clone());
        
		// `submit_unsigned_transaction` returns a type of `Result<(), ()>`
		//   ref: https://substrate.dev/rustdocs/v3.0.0/frame_system/offchain/struct.SubmitTransaction.html#method.submit_unsigned_transaction
		SubmitTransaction::<T, Call<T>>::submit_unsigned_transaction(call.into()).map_err(|_| {
            debug::error!("Failed in update_ethereum_price_worker");
			<Error<T>>::OffchainUnsignedTxError
		})
		
		// let result = SubmitTransaction::<T, Call<T>>::submit_unsigned_transaction(|_acct|
		// 	// This is the on-chain function
		// 	Call::submit_ethereum_price(final_price.clone())
		// );
			
		// 	// Display error if the signed tx fails.
		// if let Some((acc, res)) = result {
		// 	if res.is_err() {
		// 		debug::error!("failure: offchain_signed_tx: tx sent: {:?}", acc.id);
		// 		return Err(<Error<T>>::OffchainSignedTxError);
		// 	}
		// 	// Transaction is sent successfully
		// 	return Ok(());
		// } else {
		// 	// The case result == `None`: no account is available for sending
		// 	debug::error!("No local account available");
		// 	return Err(<Error<T>>::NoLocalAcctForSigning);
		// }
	}
    
	/// Fetch from remote and deserialize the JSON to a struct
	fn fetch_n_parse() -> Result<LightClient, Error<T>> {
        let resp_bytes = Self::fetch_from_remote().map_err(|e| {
            debug::error!("fetch_from_remote error: {:?}", e);
			<Error<T>>::HttpFetchingError
		})?;
        
		let resp_str = str::from_utf8(&resp_bytes).map_err(|_| <Error<T>>::HttpFetchingError)?;
		// Print out our fetched JSON string
		debug::info!("{}", resp_str);

		// Deserializing JSON to struct, thanks to `serde` and `serde_derive`
		let data: LightClient = serde_json::from_str(&resp_str).map_err(|_| <Error<T>>::HttpNotParsedInStruct)?;
		Ok(data)
	}

	/// This function uses the `offchain::http` API to query the remote blockchain,
	///   and returns the JSON response as vector of bytes.
	pub fn fetch_from_remote() -> Result<Vec<u8>, Error<T>> {
		debug::info!("sending request to: {}", HTTP_REMOTE_REQUEST);
        
		// Initiate an external HTTP GET request. This is using high-level wrappers from `sp_runtime`.
		let request = rt_offchain::http::Request::get(HTTP_REMOTE_REQUEST);
        
		// Keeping the offchain worker execution time reasonable, so limiting the call to be within 3s.
		let timeout = sp_io::offchain::timestamp()
        .add(rt_offchain::Duration::from_millis(FETCH_TIMEOUT_PERIOD));
        
		// For whatever API request, we also need to specify `user-agent` in http request header.
		let pending = request
        .add_header("User-Agent", HTTP_HEADER_USER_AGENT)
		// .add_header("Authorization", "Basic ZGhhdmFsOjEyMzQ1Njc4")
        .deadline(timeout) // Setting the timeout time
        .send() // Sending the request out by the host
        .map_err(|_| <Error<T>>::HttpFetchingError)?;
        
		//   ref: https://substrate.dev/rustdocs/v3.0.0/sp_runtime/offchain/http/struct.PendingRequest.html#method.try_wait
		let response = pending
        .try_wait(timeout)
        .map_err(|_| <Error<T>>::HttpFetchingError)?
        .map_err(|_| <Error<T>>::HttpFetchingError)?;
        
		if response.code != 200 {
            debug::error!("Unexpected http request status code: {}", response.code);
			return Err(<Error<T>>::HttpFetchingError);
		}
        
		// Next we fully read the response body and collect it to a vector of bytes.
		Ok(response.body().collect::<Vec<u8>>())
	}

	fn submt_price_to_ethereum(price: Vec<u8>, eth_host: &str, from_address: &str, to_address: &str) -> Result<Vec<u8>, Error<T>> {
		let price_in_string:&str = str::from_utf8(&price).map_err(|_| <Error<T>>::HttpFetchingError)?;
		debug::info!("Price in string{:?}",price_in_string);
		let price_float = price_in_string.parse::<f32>().map_err(|_| <Error<T>>::ParseFloatError)?;
		debug::info!("Price in float{:?}",price_float);
		let _price_int = price_float as u32;		

		let param = format!("{:x}", _price_int);

    	let data2 = format!("0xd423740b{}{}", "0".repeat(64 - param.len()), param);
		let data = data2.as_str();
		debug::info!("Data: {}",data);

		let gas_result = Self::get_estimated_gas(data, from_address, to_address).unwrap();
		let gas_result_str = str::from_utf8(&gas_result).unwrap();
		let gas_result_struct:EthResult = serde_json::from_str(gas_result_str).unwrap();


		// debug::info!("hex_price: {:?}",hex_price);

		// let (_eloop, http) = web3_rs_wasm::transports::Http::new("http://localhost:8545").unwrap();
		// let web3 = web3_rs_wasm::Web3::new(http);
		// let accounts = web3.eth().accounts().wait().unwrap();

		// println!("Accounts: {:?}", accounts);

		// let _body = "{\"jsonrpc\":\"2.0\",\"method\":\"eth_call\",\"params\":[],\"id\":1}";

		let _params = EthParams{
			from:from_address.into(),
			to:to_address.into(),
			value:"0x0".into(),
			gas:gas_result_struct.result.into(),
			gas_price:"0x1".into(),
			data:data.into(),
		};

		// let param_string = r#"{"from":"0x7e4dc815bd24ec3741b01471ffeff474cd0e0ab3","to":"0xDDb1C71FF6756F4e3f6af22e9b35BEBbebF391A0","value":"0x0","gas":"0x6800","gasPrice":"0x1","data":"0xd423740b0000000000000000000000000000000000000000000000000000000000000020"}"#;

		// let mut _params:EthParams = serde_json::from_str(param_string).map_err(|_| <Error<T>>::HttpNotParsedInStruct)?;
		// _params.data = data.into();

		// debug::info!("_param : {:?}", _params);

		// debug::info!("_param2 : {:?}", _params2);

		// let _param_json= serde_json::to_string(&_params).unwrap();
		// let _param_json = _param_json.as_str();

		
		// debug::info!("_param_json : {}", _param_json);

		let mut params_vec = Vec::new();
		params_vec.push(_params);

		let _eth_transaction = EthTransaction{
			jsonrpc:"2.0".into(),
			id:2u8,
			method:"eth_sendTransaction".into(),
			params:params_vec
		};

		// let req_string = format!("{{\"jsonrpc\":\"{jsonrpc}\",\"id\":\"{id}\",\"method\":\"{method}\",\"params\":[{{\"from\":\"{from}\",\"to\":\"{to}\",\"value\":\"{value}\",\"gas\":\"{gas}\",\"gasPrice\":\"{gas_price}\",\"data\":\"{data}\"}}]}}",
		// 	jsonrpc = "2.0",
		// 	id = 1,
		// 	method = "eth_sendTransaction",
		// 	from="0x7e4dc815bd24ec3741b01471ffeff474cd0e0ab3",
		// 	to="0x193dc3d481dff990c4885cf305b5b930b9e5f818",
		// 	gas = "0x0",
		// 	gas_price="0x1",
		// 	data = "0xd423740b0000000000000000000000000000000000000000000000000000000000000019",
		// );

		// let req_body = br#"{"jsonrpc":"2.0","id":"2","method":"eth_sendTransaction","params":[{"from":"0x7e4dc815bd24ec3741b01471ffeff474cd0e0ab3","to":"0x193dc3d481dff990c4885cf305b5b930b9e5f818","value":"0x0","gas":"0x5d68","gasPrice":"0x1","data":"0xd423740b0000000000000000000000000000000000000000000000000000000000000019"}]}"#;
		// let req_body = br#"req_string"#;

		let _json_data = serde_json::to_vec(&_eth_transaction).map_err(|_| <Error<T>>::HttpNotParsedInStruct)?;
		let _body:&[u8] = &_json_data;
		// let bbody = &*_body.clone();
		let body_str = str::from_utf8(_body).map_err(|_| <Error<T>>::HttpFetchingError)?;

		debug::info!("jsonString: {:?}", body_str);

		let request = rt_offchain::http::Request::post(eth_host,vec![_body]);
        
		// Keeping the offchain worker execution time reasonable, so limiting the call to be within 3s.
		let timeout = sp_io::offchain::timestamp()
        .add(rt_offchain::Duration::from_millis(FETCH_TIMEOUT_PERIOD));
        
		// // For whatever API request, we also need to specify `user-agent` in http request header.
		let pending = request
        .add_header("Content-Type", "application/json")
		// // .add_header("Authorization", "Basic ZGhhdmFsOjEyMzQ1Njc4")
        .deadline(timeout) // Setting the timeout time
        .send() // Sending the request out by the host
        .map_err(|_| <Error<T>>::HttpFetchingError)?;
        
		// //   ref: https://substrate.dev/rustdocs/v3.0.0/sp_runtime/offchain/http/struct.PendingRequest.html#method.try_wait
		let response = pending
        .try_wait(timeout)
        .map_err(|_| <Error<T>>::HttpFetchingError)?
        .map_err(|_| <Error<T>>::HttpFetchingError)?;
        
		if response.code != 200 {
            debug::error!("Unexpected http request status code: {}", response.code);
			return Err(<Error<T>>::HttpFetchingError);
		}
        
		// // Next we fully read the response body and collect it to a vector of bytes.
		Ok(response.body().collect::<Vec<u8>>())
	}

	fn get_estimated_gas(data:&str, from_address: &str, to_address: &str) -> Result<Vec<u8>, Error<T>> {
		debug::info!("Sending request to for gas price: {}", HTTP_ETHEREUM_HOST);

		let gas_request_param = GasRequestParam{
			from:from_address.into(),
			to:to_address.into(),
			value:"0x0".into(),
			data: data.into(),
		};

		let mut param_vec = Vec::new();
		param_vec.push(gas_request_param);

		let estimate_gas_req = EstimateGasRequest {
			jsonrpc : "2.0".into(),
			id:20u8,
			method:"eth_estimateGas".into(),
			params:param_vec,
		};

		let _json_data = serde_json::to_vec(&estimate_gas_req).map_err(|_| <Error<T>>::HttpNotParsedInStruct)?;
		let _body:&[u8] = &_json_data;

		let body_str = str::from_utf8(_body).map_err(|_| <Error<T>>::HttpFetchingError)?;

		debug::info!("gas request jsonString: {:?}", body_str);
        
		// Initiate an external HTTP GET request. This is using high-level wrappers from `sp_runtime`.
		let request = rt_offchain::http::Request::post(HTTP_ETHEREUM_HOST, vec![_body]);
        
		// Keeping the offchain worker execution time reasonable, so limiting the call to be within 3s.
		let timeout = sp_io::offchain::timestamp()
        .add(rt_offchain::Duration::from_millis(FETCH_TIMEOUT_PERIOD));
        
		let pending = request
		.add_header("Content-Type", "application/json")
        .deadline(timeout) // Setting the timeout time
        .send() // Sending the request out by the host
        .map_err(|_| <Error<T>>::HttpFetchingError)?;
        
		let response = pending
        .try_wait(timeout)
        .map_err(|_| <Error<T>>::HttpFetchingError)?
        .map_err(|_| <Error<T>>::HttpFetchingError)?;
        
		if response.code != 200 {
            debug::error!("Unexpected http request status code for gas price: {}", response.code);
			return Err(<Error<T>>::HttpFetchingError);
		}
        
		// Next we fully read the response body and collect it to a vector of bytes.
		Ok(response.body().collect::<Vec<u8>>())
	}
}

impl<T: Config> frame_support::unsigned::ValidateUnsigned for Module<T> {
    type Call = Call<T>;
    
	fn validate_unsigned(_source: TransactionSource, call: &Self::Call) -> TransactionValidity {
        let valid_tx = |provide| {
            ValidTransaction::with_tag_prefix("ocw-demo") //TODO: change prefix tag
            .priority(UNSIGNED_TXS_PRIORITY)
            .and_provides([&provide])
            .longevity(3)
            .propagate(true)
				.build()
            };
            
            match call {
                //Call::submit_number_unsigned(_number) => valid_tx(b"submit_number_unsigned".to_vec()),

                //Call::submit_number_unsigned_with_signed_payload(ref payload, ref signature) => {
                //    if !SignedPayload::<T>::verify::<T::AuthorityId>(payload, signature.clone()) {
                //        return InvalidTransaction::BadProof.into();
                //    }
                //    valid_tx(b"submit_number_unsigned_with_signed_payload".to_vec())
                //},

				Call::submit_ethereum_price(_ethereum_price) => valid_tx(b"submit_ethereum_price".to_vec()),
                
                _ => InvalidTransaction::Call.into(),
            }
	}
}

impl<T: Config> rt_offchain::storage_lock::BlockNumberProvider for Module<T> {
    type BlockNumber = T::BlockNumber;
	fn current_block_number() -> Self::BlockNumber {
        <frame_system::Module<T>>::block_number()
	}
}

#[derive(Debug, Deserialize, Encode, Decode, Default)]
struct IndexingData(Vec<u8>, u64);

#[derive(Debug, Deserialize, Encode, Decode, Default)]
struct IndexingPriceData(Vec<u8>, Vec<u8>);

#[derive(Debug, Deserialize, Encode, Decode, Default)]
struct IndexingPriceFlag(Vec<u8>);

#[derive(Deserialize, Encode, Decode, Default)]
struct LightClient {
    // Specify our own deserializing function to convert JSON string to vector of bytes
	// #[serde(deserialize_with = "de_string_to_bytes")]
	// header: Vec<u8>,
	// block: u8,
    // public_repos: u32,
	// {"trade_id":123560925,"price":"2726.25","size":"0.07600729","time":"2021-05-27T06:28:47.05024Z","bid":"2726.73","ask":"2726.74","volume":"517961.94817769"}
	#[serde(deserialize_with = "de_string_to_bytes")]
	price: Vec<u8>,
	#[serde(deserialize_with = "de_string_to_bytes")]
	time: Vec<u8>,
}

pub fn de_string_to_bytes<'de, D>(de: D) -> Result<Vec<u8>, D::Error>
where D: Deserializer<'de> {
    let s: &str = Deserialize::deserialize(de)?;
    Ok(s.as_bytes().to_vec())
}

impl fmt::Debug for LightClient {
	// `fmt` converts the vector of bytes inside the struct back to string for
	//   more friendly display.
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			f,
			"{{ price: {}, time: {} }}",
			// &self.price,
			str::from_utf8(&self.price).map_err(|_| fmt::Error)?,
			str::from_utf8(&self.time).map_err(|_| fmt::Error)?,
			// &self.time,
		)
	}
}

#[derive(Deserialize, Encode, Decode, Default)]
struct EthTransaction {
    // Specify our own deserializing function to convert JSON string to vector of bytes
	#[serde(deserialize_with = "de_string_to_bytes")]
	jsonrpc: Vec<u8>,
	id: u8,
	#[serde(deserialize_with = "de_string_to_bytes")]
	method: Vec<u8>,
	params: Vec<EthParams>
}

#[derive(Deserialize, Encode, Decode, Default)]
struct EthParams {
    // Specify our own deserializing function to convert JSON string to vector of bytes
	#[serde(deserialize_with = "de_string_to_bytes")]
	from: Vec<u8>,
	#[serde(deserialize_with = "de_string_to_bytes")]
	to: Vec<u8>,
	#[serde(deserialize_with = "de_string_to_bytes")]
	data: Vec<u8>,
	#[serde(deserialize_with = "de_string_to_bytes")]
	value: Vec<u8>,
	#[serde(deserialize_with = "de_string_to_bytes")]
	gas: Vec<u8>,
	#[serde(deserialize_with = "de_string_to_bytes",rename="gasPrice")]
	gas_price: Vec<u8>,
}

#[derive(Deserialize, Encode, Decode, Default)]
struct EstimateGasRequest {
    // Specify our own deserializing function to convert JSON string to vector of bytes
	#[serde(deserialize_with = "de_string_to_bytes")]
	jsonrpc: Vec<u8>,
	id: u8,
	#[serde(deserialize_with = "de_string_to_bytes")]
	method: Vec<u8>,
	params: Vec<GasRequestParam>,
}

#[derive(Deserialize, Encode, Decode, Default)]
struct GasRequestParam {
    // Specify our own deserializing function to convert JSON string to vector of bytes
	#[serde(deserialize_with = "de_string_to_bytes")]
	from: Vec<u8>,
	#[serde(deserialize_with = "de_string_to_bytes")]
	to: Vec<u8>,
	#[serde(deserialize_with = "de_string_to_bytes")]
	data: Vec<u8>,
	#[serde(deserialize_with = "de_string_to_bytes")]
	value: Vec<u8>,
}

impl fmt::Debug for EthTransaction {
	// `fmt` converts the vector of bytes inside the struct back to string for
	//   more friendly display.
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			f,
			"{{ jsonrpc: {}, id: {},method: {},params: [{:?}]  }}",
			// &self.price,
			str::from_utf8(&self.jsonrpc).map_err(|_| fmt::Error)?,
			&self.id,
			str::from_utf8(&self.method).map_err(|_| fmt::Error)?,
			&self.params,
			// &self.time,
		)
	}
}

impl fmt::Debug for EthParams {
	// `fmt` converts the vector of bytes inside the struct back to string for
	//   more friendly display.
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			f,
			"{{ from: {}, to: {}, data: {}, value: {},gas: {}, gasPrice: {} }}",
			// &self.price,
			str::from_utf8(&self.from).map_err(|_| fmt::Error)?,
			str::from_utf8(&self.to).map_err(|_| fmt::Error)?,
			str::from_utf8(&self.data).map_err(|_| fmt::Error)?,
			str::from_utf8(&self.value).map_err(|_| fmt::Error)?,
			str::from_utf8(&self.gas).map_err(|_| fmt::Error)?,
			str::from_utf8(&self.gas_price).map_err(|_| fmt::Error)?,
			// &self.time,
		)
	}
}

impl fmt::Debug for EstimateGasRequest {
	// `fmt` converts the vector of bytes inside the struct back to string for
	//   more friendly display.
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			f,
			"{{ jsonrpc: {}, id: {},method: {},params: [{:?}]  }}",
			// &self.price,
			str::from_utf8(&self.jsonrpc).map_err(|_| fmt::Error)?,
			&self.id,
			str::from_utf8(&self.method).map_err(|_| fmt::Error)?,
			&self.params,
			// &self.time,
		)
	}
}

impl fmt::Debug for GasRequestParam {
	// `fmt` converts the vector of bytes inside the struct back to string for
	//   more friendly display.
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			f,
			"{{ from: {}, to: {}, data: {}, value: {} }}",
			// &self.price,
			str::from_utf8(&self.from).map_err(|_| fmt::Error)?,
			str::from_utf8(&self.to).map_err(|_| fmt::Error)?,
			str::from_utf8(&self.data).map_err(|_| fmt::Error)?,
			str::from_utf8(&self.value).map_err(|_| fmt::Error)?,
			// &self.time,
		)
	}
}

#[derive(Deserialize, Serialize, Encode, Decode, Default)]
struct EthResult {
    // Specify our own deserializing function to convert JSON string to vector of bytes
	#[serde(deserialize_with = "de_string_to_bytes")]
	jsonrpc: Vec<u8>,
	id: u8,
	#[serde(deserialize_with = "de_string_to_bytes")]
	result: Vec<u8>,	
}

impl fmt::Debug for EthResult {
	// `fmt` converts the vector of bytes inside the struct back to string for
	//   more friendly display.
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			f,
			"{{ jsonrpc: {}, id: {},result: {} }}",
			// &self.price,
			str::from_utf8(&self.jsonrpc).map_err(|_| fmt::Error)?,
			&self.id,
			str::from_utf8(&self.result).map_err(|_| fmt::Error)?,
			// &self.params,
			// &self.time,
		)
	}
}

impl Serialize for EthParams {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // 6 is the number of fields in the struct.
        let mut state = serializer.serialize_struct("EthParams", 6)?;
        state.serialize_field("from", str::from_utf8(&self.from).unwrap())?;
        state.serialize_field("to", str::from_utf8(&self.to).unwrap())?;
        state.serialize_field("data", str::from_utf8(&self.data).unwrap())?;
		state.serialize_field("value", str::from_utf8(&self.value).unwrap())?;
        state.serialize_field("gas", str::from_utf8(&self.gas).unwrap())?;
        state.serialize_field("gasPrice", str::from_utf8(&self.gas_price).unwrap())?;
        state.end()
    }
}

impl Serialize for EthTransaction {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // 6 is the number of fields in the struct.
        let mut state = serializer.serialize_struct("EthParams", 4)?;
        state.serialize_field("jsonrpc", str::from_utf8(&self.jsonrpc).unwrap())?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("method", str::from_utf8(&self.method).unwrap())?;
		state.serialize_field("params", &self.params)?;
        state.end()
    }
}

impl Serialize for EstimateGasRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // 6 is the number of fields in the struct.
        let mut state = serializer.serialize_struct("EthParams", 4)?;
        state.serialize_field("jsonrpc", str::from_utf8(&self.jsonrpc).unwrap())?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("method", str::from_utf8(&self.method).unwrap())?;
		state.serialize_field("params", &self.params)?;
        state.end()
    }
}

impl Serialize for GasRequestParam {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // 6 is the number of fields in the struct.
        let mut state = serializer.serialize_struct("EthParams", 4)?;
        state.serialize_field("from", str::from_utf8(&self.from).unwrap())?;
        state.serialize_field("to", str::from_utf8(&self.to).unwrap())?;
        state.serialize_field("data", str::from_utf8(&self.data).unwrap())?;
		state.serialize_field("value", str::from_utf8(&self.value).unwrap())?;
        state.end()
    }
}
