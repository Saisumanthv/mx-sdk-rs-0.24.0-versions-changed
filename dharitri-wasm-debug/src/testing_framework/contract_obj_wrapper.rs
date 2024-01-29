use std::{collections::HashMap, path::PathBuf, rc::Rc, str::FromStr};

use dharitri_wasm::{
    contract_base::{CallableContract, ContractBase},
    types::{Address, DctLocalRole, H256},
};

use crate::{
    rust_biguint,
    testing_framework::bytes_to_hex,
    tx_mock::{TxCache, TxContext, TxContextStack, TxInput, TxInputDCT},
    world_mock::{AccountData, AccountDct, DctInstanceMetadata},
    BlockchainMock, DebugApi,
};

use super::{
    tx_denali::{ScCallDenali, TxExpectDenali},
    AddressFactory, DenaliGenerator, ScQueryDenali,
};

pub struct ContractObjWrapper<
    CB: ContractBase<Api = DebugApi> + CallableContract<DebugApi> + 'static,
    ContractObjBuilder: 'static + Copy + Fn(DebugApi) -> CB,
> {
    pub(crate) address: Address,
    pub(crate) obj_builder: ContractObjBuilder,
}

impl<CB, ContractObjBuilder> ContractObjWrapper<CB, ContractObjBuilder>
where
    CB: ContractBase<Api = DebugApi> + CallableContract<DebugApi> + 'static,
    ContractObjBuilder: 'static + Copy + Fn(DebugApi) -> CB,
{
    pub(crate) fn new(address: Address, obj_builder: ContractObjBuilder) -> Self {
        ContractObjWrapper {
            address,
            obj_builder,
        }
    }

    pub fn address_ref(&self) -> &Address {
        &self.address
    }
}

pub struct BlockchainStateWrapper {
    address_factory: AddressFactory,
    rc_b_mock: Rc<BlockchainMock>,
    address_to_code_path: HashMap<Address, Vec<u8>>,
    denali_generator: DenaliGenerator,
    workspace_path: PathBuf,
}

pub enum StateChange {
    Commit,
    Revert,
}

impl BlockchainStateWrapper {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let mut current_dir = std::env::current_dir().unwrap();
        current_dir.push(PathBuf::from_str("denali/").unwrap());

        BlockchainStateWrapper {
            address_factory: AddressFactory::new(),
            rc_b_mock: Rc::new(BlockchainMock::new()),
            address_to_code_path: HashMap::new(),
            denali_generator: DenaliGenerator::new(),
            workspace_path: current_dir,
        }
    }

    pub fn write_denali_output(self, file_name: &str) {
        let mut full_path = self.workspace_path;
        full_path.push(file_name);

        self.denali_generator
            .write_denali_output(full_path.to_str().unwrap());
    }

    pub fn check_moax_balance(&self, address: &Address, expected_balance: &num_bigint::BigUint) {
        let actual_balance = match &self.rc_b_mock.accounts.get(address) {
            Some(acc) => acc.moax_balance.clone(),
            None => rust_biguint!(0),
        };

        assert_eq!(
            expected_balance,
            &actual_balance,
            "MOAX balance mismatch for address {}. Expected: {}, have {}",
            address_to_hex(address),
            expected_balance,
            actual_balance
        );
    }

    pub fn check_dct_balance(
        &self,
        address: &Address,
        token_id: &[u8],
        expected_balance: &num_bigint::BigUint,
    ) {
        let actual_balance = match &self.rc_b_mock.accounts.get(address) {
            Some(acc) => acc.dct.get_dct_balance(token_id, 0),
            None => rust_biguint!(0),
        };

        assert_eq!(
            expected_balance,
            &actual_balance,
            "DCT balance mismatch for address {}. Expected: {}, have {}",
            address_to_hex(address),
            expected_balance,
            actual_balance
        );
    }

    pub fn check_nft_balance<T: dharitri_wasm::dharitri_codec::TopEncode>(
        &self,
        address: &Address,
        token_id: &[u8],
        nonce: u64,
        expected_balance: &num_bigint::BigUint,
        expected_attributes: &T,
    ) {
        let actual_attributes = match &self.rc_b_mock.accounts.get(address) {
            Some(acc) => {
                let dct_data = acc.dct.get_by_identifier_or_default(token_id);
                let opt_instance = dct_data.instances.get_by_nonce(nonce);

                match opt_instance {
                    Some(instance) => {
                        assert_eq!(
                            expected_balance,
                            &instance.balance,
                            "DCT NFT balance mismatch for address {}. Expected: {}, have {}",
                            address_to_hex(address),
                            expected_balance,
                            instance.balance
                        );

                        instance.metadata.attributes.clone()
                    },
                    None => Vec::new(),
                }
            },
            None => Vec::new(),
        };

        let serialized_expected = serialize_attributes(expected_attributes);
        assert_eq!(
            &serialized_expected,
            &actual_attributes,
            "DCT NFT attributes mismatch for address {}. Expected: {}, have {}",
            address_to_hex(address),
            bytes_to_hex(&serialized_expected),
            bytes_to_hex(&actual_attributes),
        );
    }

    /*
    pub fn check_nft_balance_with_properties(
        &self,
        address: &Address,
        token_id: &[u8],
        nonce: u64,
        expected_balance: &num_bigint::BigUint,
    ) {
    }
    */
}

impl BlockchainStateWrapper {
    pub fn create_user_account(&mut self, moax_balance: &num_bigint::BigUint) -> Address {
        let address = self.address_factory.new_address();
        self.create_account_raw(&address, moax_balance, None, None, None);

        address
    }

    pub fn create_sc_account<CB, ContractObjBuilder>(
        &mut self,
        moax_balance: &num_bigint::BigUint,
        owner: Option<&Address>,
        obj_builder: ContractObjBuilder,
        contract_wasm_path: &str,
    ) -> ContractObjWrapper<CB, ContractObjBuilder>
    where
        CB: ContractBase<Api = DebugApi> + CallableContract<DebugApi> + 'static,
        ContractObjBuilder: 'static + Copy + Fn(DebugApi) -> CB,
    {
        let address = self.address_factory.new_sc_address();

        let mut wasm_full_path = std::env::current_dir().unwrap();
        wasm_full_path.push(PathBuf::from_str(contract_wasm_path).unwrap());

        let path_diff =
            pathdiff::diff_paths(wasm_full_path.clone(), self.workspace_path.clone()).unwrap();
        let path_str = path_diff.to_str().unwrap();

        let wasm_full_path_as_expr = "file:".to_owned() + wasm_full_path.to_str().unwrap();
        let contract_bytes = denali::value_interpreter::interpret_string(
            &wasm_full_path_as_expr,
            &denali::interpret_trait::InterpreterContext::new(std::path::PathBuf::new()),
        );

        let wasm_relative_path_expr = "file:".to_owned() + path_str;
        let was_relative_path_expr_bytes = wasm_relative_path_expr.as_bytes().to_vec();

        self.address_to_code_path
            .insert(address.clone(), was_relative_path_expr_bytes.clone());

        self.create_account_raw(
            &address,
            moax_balance,
            owner,
            Some(contract_bytes),
            Some(was_relative_path_expr_bytes),
        );

        if !self.rc_b_mock.contains_contract(&wasm_full_path_as_expr) {
            let closure = convert_full_fn(obj_builder);

            let b_mock_ref = Rc::get_mut(&mut self.rc_b_mock).unwrap();
            b_mock_ref.register_contract(&wasm_full_path_as_expr, closure);
        }

        ContractObjWrapper::new(address, obj_builder)
    }

    pub fn create_account_raw(
        &mut self,
        address: &Address,
        moax_balance: &num_bigint::BigUint,
        owner: Option<&Address>,
        sc_identifier: Option<Vec<u8>>,
        sc_denali_path_expr: Option<Vec<u8>>,
    ) {
        let acc_data = AccountData {
            address: address.clone(),
            nonce: 0,
            moax_balance: moax_balance.clone(),
            dct: AccountDct::default(),
            storage: HashMap::new(),
            username: Vec::new(),
            contract_path: sc_identifier,
            contract_owner: owner.cloned(),
        };
        self.denali_generator
            .set_account(&acc_data, sc_denali_path_expr);

        let b_mock_ref = Rc::get_mut(&mut self.rc_b_mock).unwrap();
        b_mock_ref.add_account(acc_data);
    }

    pub fn set_moax_balance(&mut self, address: &Address, balance: &num_bigint::BigUint) {
        let b_mock_ref = Rc::get_mut(&mut self.rc_b_mock).unwrap();
        match b_mock_ref.accounts.get_mut(address) {
            Some(acc) => {
                acc.moax_balance = balance.clone();

                self.add_denali_set_account(address);
            },

            None => panic!(
                "set_moax_balance: Account {:?} does not exist",
                address_to_hex(address)
            ),
        }
    }

    pub fn set_dct_balance(
        &mut self,
        address: &Address,
        token_id: &[u8],
        balance: &num_bigint::BigUint,
    ) {
        let b_mock_ref = Rc::get_mut(&mut self.rc_b_mock).unwrap();
        match b_mock_ref.accounts.get_mut(address) {
            Some(acc) => {
                acc.dct.set_dct_balance(
                    token_id.to_vec(),
                    0,
                    balance,
                    DctInstanceMetadata::default(),
                );
            },
            None => panic!(
                "set_dct_balance: Account {:?} does not exist",
                address_to_hex(address)
            ),
        }
    }

    pub fn set_nft_balance<T: dharitri_wasm::dharitri_codec::TopEncode>(
        &mut self,
        address: &Address,
        token_id: &[u8],
        nonce: u64,
        balance: &num_bigint::BigUint,
        attributes: &T,
    ) {
        self.set_nft_balance_all_properties(
            address, token_id, nonce, balance, attributes, 0, None, None, None, None,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_nft_balance_all_properties<T: dharitri_wasm::dharitri_codec::TopEncode>(
        &mut self,
        address: &Address,
        token_id: &[u8],
        nonce: u64,
        balance: &num_bigint::BigUint,
        attributes: &T,
        royalties: u64,
        creator: Option<&Address>,
        name: Option<&[u8]>,
        hash: Option<&[u8]>,
        uri: Option<&[u8]>,
    ) {
        let b_mock_ref = Rc::get_mut(&mut self.rc_b_mock).unwrap();
        match b_mock_ref.accounts.get_mut(address) {
            Some(acc) => {
                acc.dct.set_dct_balance(
                    token_id.to_vec(),
                    nonce,
                    balance,
                    DctInstanceMetadata {
                        creator: creator.cloned(),
                        attributes: serialize_attributes(attributes),
                        royalties,
                        name: name.unwrap_or_default().to_vec(),
                        hash: hash.map(|h| h.to_vec()),
                        uri: uri.map(|u| u.to_vec()),
                    },
                );

                self.add_denali_set_account(address);
            },
            None => panic!(
                "set_nft_balance: Account {:?} does not exist",
                address_to_hex(address)
            ),
        }
    }

    pub fn set_dct_local_roles(
        &mut self,
        address: &Address,
        token_id: &[u8],
        roles: &[DctLocalRole],
    ) {
        let b_mock_ref = Rc::get_mut(&mut self.rc_b_mock).unwrap();
        match b_mock_ref.accounts.get_mut(address) {
            Some(acc) => {
                let mut roles_raw = Vec::new();
                for role in roles {
                    roles_raw.push(role.as_role_name().to_vec());
                }
                acc.dct.set_roles(token_id.to_vec(), roles_raw);

                self.add_denali_set_account(address);
            },
            None => panic!(
                "set_dct_local_roles: Account {:?} does not exist",
                address_to_hex(address)
            ),
        }
    }

    pub fn set_block_epoch(&mut self, block_epoch: u64) {
        let b_mock_ref = Rc::get_mut(&mut self.rc_b_mock).unwrap();
        b_mock_ref.current_block_info.block_epoch = block_epoch;

        self.denali_generator.set_block_info(
            &self.rc_b_mock.current_block_info,
            &self.rc_b_mock.previous_block_info,
        );
    }

    pub fn set_block_nonce(&mut self, block_nonce: u64) {
        let b_mock_ref = Rc::get_mut(&mut self.rc_b_mock).unwrap();
        b_mock_ref.current_block_info.block_nonce = block_nonce;

        self.denali_generator.set_block_info(
            &self.rc_b_mock.current_block_info,
            &self.rc_b_mock.previous_block_info,
        );
    }

    pub fn set_block_random_seed(&mut self, block_random_seed: Box<[u8; 48]>) {
        let b_mock_ref = Rc::get_mut(&mut self.rc_b_mock).unwrap();
        b_mock_ref.current_block_info.block_random_seed = block_random_seed;

        self.denali_generator.set_block_info(
            &self.rc_b_mock.current_block_info,
            &self.rc_b_mock.previous_block_info,
        );
    }

    pub fn set_block_round(&mut self, block_round: u64) {
        let b_mock_ref = Rc::get_mut(&mut self.rc_b_mock).unwrap();
        b_mock_ref.current_block_info.block_round = block_round;

        self.denali_generator.set_block_info(
            &self.rc_b_mock.current_block_info,
            &self.rc_b_mock.previous_block_info,
        );
    }

    pub fn set_block_timestamp(&mut self, block_timestamp: u64) {
        let b_mock_ref = Rc::get_mut(&mut self.rc_b_mock).unwrap();
        b_mock_ref.current_block_info.block_timestamp = block_timestamp;

        self.denali_generator.set_block_info(
            &self.rc_b_mock.current_block_info,
            &self.rc_b_mock.previous_block_info,
        );
    }

    pub fn set_prev_block_epoch(&mut self, block_epoch: u64) {
        let b_mock_ref = Rc::get_mut(&mut self.rc_b_mock).unwrap();
        b_mock_ref.previous_block_info.block_epoch = block_epoch;

        self.denali_generator.set_block_info(
            &self.rc_b_mock.current_block_info,
            &self.rc_b_mock.previous_block_info,
        );
    }

    pub fn set_prev_block_nonce(&mut self, block_nonce: u64) {
        let b_mock_ref = Rc::get_mut(&mut self.rc_b_mock).unwrap();
        b_mock_ref.previous_block_info.block_nonce = block_nonce;

        self.denali_generator.set_block_info(
            &self.rc_b_mock.current_block_info,
            &self.rc_b_mock.previous_block_info,
        );
    }

    pub fn set_prev_block_random_seed(&mut self, block_random_seed: Box<[u8; 48]>) {
        let b_mock_ref = Rc::get_mut(&mut self.rc_b_mock).unwrap();
        b_mock_ref.previous_block_info.block_random_seed = block_random_seed;

        self.denali_generator.set_block_info(
            &self.rc_b_mock.current_block_info,
            &self.rc_b_mock.previous_block_info,
        );
    }

    pub fn set_prev_block_round(&mut self, block_round: u64) {
        let b_mock_ref = Rc::get_mut(&mut self.rc_b_mock).unwrap();
        b_mock_ref.previous_block_info.block_round = block_round;

        self.denali_generator.set_block_info(
            &self.rc_b_mock.current_block_info,
            &self.rc_b_mock.previous_block_info,
        );
    }

    pub fn set_prev_block_timestamp(&mut self, block_timestamp: u64) {
        let b_mock_ref = Rc::get_mut(&mut self.rc_b_mock).unwrap();
        b_mock_ref.previous_block_info.block_timestamp = block_timestamp;

        self.denali_generator.set_block_info(
            &self.rc_b_mock.current_block_info,
            &self.rc_b_mock.previous_block_info,
        );
    }

    pub fn add_denali_sc_call(
        &mut self,
        sc_call: ScCallDenali,
        opt_expect: Option<TxExpectDenali>,
    ) {
        self.denali_generator
            .create_tx(&sc_call, opt_expect.as_ref());
    }

    pub fn add_denali_sc_query(
        &mut self,
        sc_query: ScQueryDenali,
        opt_expect: Option<TxExpectDenali>,
    ) {
        self.denali_generator
            .create_query(&sc_query, opt_expect.as_ref());
    }

    pub fn add_denali_set_account(&mut self, address: &Address) {
        if let Some(acc) = self.rc_b_mock.accounts.get(address) {
            let opt_contract_path = self.address_to_code_path.get(address);
            self.denali_generator
                .set_account(acc, opt_contract_path.cloned());
        }
    }

    pub fn add_denali_check_account(&mut self, address: &Address) {
        if let Some(acc) = self.rc_b_mock.accounts.get(address) {
            self.denali_generator.check_account(acc);
        }
    }
}

impl BlockchainStateWrapper {
    pub fn execute_tx<CB, ContractObjBuilder, TxFn: FnOnce(CB) -> StateChange>(
        &mut self,
        caller: &Address,
        sc_wrapper: &ContractObjWrapper<CB, ContractObjBuilder>,
        moax_payment: &num_bigint::BigUint,
        tx_fn: TxFn,
    ) where
        CB: ContractBase<Api = DebugApi> + CallableContract<DebugApi> + 'static,
        ContractObjBuilder: 'static + Copy + Fn(DebugApi) -> CB,
    {
        self.execute_tx_any(caller, sc_wrapper, moax_payment, Vec::new(), tx_fn);
    }

    pub fn execute_dct_transfer<CB, ContractObjBuilder, TxFn: FnOnce(CB) -> StateChange>(
        &mut self,
        caller: &Address,
        sc_wrapper: &ContractObjWrapper<CB, ContractObjBuilder>,
        token_id: &[u8],
        dct_nonce: u64,
        dct_amount: &num_bigint::BigUint,
        tx_fn: TxFn,
    ) where
        CB: ContractBase<Api = DebugApi> + CallableContract<DebugApi> + 'static,
        ContractObjBuilder: 'static + Copy + Fn(DebugApi) -> CB,
    {
        let dct_transfer = vec![TxInputDCT {
            token_identifier: token_id.to_vec(),
            nonce: dct_nonce,
            value: dct_amount.clone(),
        }];
        self.execute_tx_any(caller, sc_wrapper, &rust_biguint!(0), dct_transfer, tx_fn);
    }

    pub fn execute_dct_multi_transfer<CB, ContractObjBuilder, TxFn: FnOnce(CB) -> StateChange>(
        &mut self,
        caller: &Address,
        sc_wrapper: &ContractObjWrapper<CB, ContractObjBuilder>,
        dct_transfers: &[TxInputDCT],
        tx_fn: TxFn,
    ) where
        CB: ContractBase<Api = DebugApi> + CallableContract<DebugApi> + 'static,
        ContractObjBuilder: 'static + Copy + Fn(DebugApi) -> CB,
    {
        self.execute_tx_any(
            caller,
            sc_wrapper,
            &rust_biguint!(0),
            dct_transfers.to_vec(),
            tx_fn,
        );
    }

    pub fn execute_query<CB, ContractObjBuilder, TxFn: FnOnce(CB)>(
        &mut self,
        sc_wrapper: &ContractObjWrapper<CB, ContractObjBuilder>,
        query_fn: TxFn,
    ) where
        CB: ContractBase<Api = DebugApi> + CallableContract<DebugApi> + 'static,
        ContractObjBuilder: 'static + Copy + Fn(DebugApi) -> CB,
    {
        self.execute_tx(
            sc_wrapper.address_ref(),
            sc_wrapper,
            &rust_biguint!(0),
            |sc| {
                query_fn(sc);
                StateChange::Revert
            },
        );
    }

    // deduplicates code for execution
    fn execute_tx_any<CB, ContractObjBuilder, TxFn: FnOnce(CB) -> StateChange>(
        &mut self,
        caller: &Address,
        sc_wrapper: &ContractObjWrapper<CB, ContractObjBuilder>,
        moax_payment: &num_bigint::BigUint,
        dct_payments: Vec<TxInputDCT>,
        tx_fn: TxFn,
    ) where
        CB: ContractBase<Api = DebugApi> + CallableContract<DebugApi> + 'static,
        ContractObjBuilder: 'static + Copy + Fn(DebugApi) -> CB,
    {
        let sc_address = sc_wrapper.address_ref();
        let tx_cache = TxCache::new(self.rc_b_mock.clone());
        let rust_zero = rust_biguint!(0);

        if moax_payment > &rust_zero {
            tx_cache.subtract_moax_balance(caller, moax_payment);
            tx_cache.increase_moax_balance(sc_address, moax_payment);
        }

        for dct in &dct_payments {
            if dct.value > rust_zero {
                let metadata = tx_cache.subtract_dct_balance(
                    caller,
                    &dct.token_identifier,
                    dct.nonce,
                    &dct.value,
                );
                tx_cache.increase_dct_balance(
                    sc_address,
                    &dct.token_identifier,
                    dct.nonce,
                    &dct.value,
                    metadata,
                );
            }
        }

        let tx_input = build_tx_input(caller, sc_address, moax_payment, dct_payments);
        let tx_context_rc = Rc::new(TxContext::new(tx_input, tx_cache));
        TxContextStack::static_push(tx_context_rc.clone());

        let debug_api = DebugApi::new(tx_context_rc);
        let sc = (sc_wrapper.obj_builder)(debug_api);

        let state_change = tx_fn(sc);

        let api_after_exec = Rc::try_unwrap(TxContextStack::static_pop()).unwrap();
        let updates = api_after_exec.into_blockchain_updates();

        let b_mock_ref = Rc::get_mut(&mut self.rc_b_mock).unwrap();
        match state_change {
            StateChange::Commit => {
                updates.apply(b_mock_ref);
            },
            StateChange::Revert => {},
        }
    }
}

fn build_tx_input(
    caller: &Address,
    dest: &Address,
    moax_value: &num_bigint::BigUint,
    dct_values: Vec<TxInputDCT>,
) -> TxInput {
    TxInput {
        from: caller.clone(),
        to: dest.clone(),
        moax_value: moax_value.clone(),
        dct_values,
        func_name: Vec::new(),
        args: Vec::new(),
        gas_limit: u64::MAX,
        gas_price: 0,
        tx_hash: H256::zero(),
    }
}

fn address_to_hex(address: &Address) -> String {
    hex::encode(address.as_bytes())
}

fn serialize_attributes<T: dharitri_wasm::dharitri_codec::TopEncode>(attributes: &T) -> Vec<u8> {
    let mut serialized_attributes = Vec::new();
    if let Result::Err(err) = attributes.top_encode(&mut serialized_attributes) {
        panic!("Failed to encode attributes: {:?}", err)
    }

    serialized_attributes
}

fn convert_full_fn<CB, ContractObjBuilder>(
    func: ContractObjBuilder,
) -> Box<dyn Fn(DebugApi) -> Box<dyn CallableContract<DebugApi>>>
where
    CB: ContractBase<Api = DebugApi> + CallableContract<DebugApi> + 'static,
    ContractObjBuilder: 'static + Fn(DebugApi) -> CB,
{
    let raw_closure = move |context| convert_part(func(context));

    Box::new(raw_closure)
}

fn convert_part<CB>(c_base: CB) -> Box<dyn CallableContract<DebugApi>>
where
    CB: ContractBase<Api = DebugApi> + CallableContract<DebugApi> + 'static,
{
    Box::new(c_base)
}
