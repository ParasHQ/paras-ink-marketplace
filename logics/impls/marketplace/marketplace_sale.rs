// Copyright (c) 2022 Astar Network
//
// Permission is hereby granted, free of charge, to any person obtaining
// a copy of this software and associated documentation files (the"Software"),
// to deal in the Software without restriction, including
// without limitation the rights to use, copy, modify, merge, publish,
// distribute, sublicense, and/or sell copies of the Software, and to
// permit persons to whom the Software is furnished to do so, subject to
// the following conditions:
//
// The above copyright notice and this permission notice shall be
// included in all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
// EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
// MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
// NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE
// LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
// WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

use super::types::{NftContractType, OfferItem, RegisteredCollection};
use crate::{
    ensure,
    impls::marketplace::types::{Data, Item, MarketplaceError},
    traits::marketplace::MarketplaceSale,
};
use openbrush::{
    contracts::{ownable::*, psp34::*, reentrancy_guard::*},
    modifiers,
    traits::{AccountId, Balance, Hash, Storage, String},
};

pub trait Internal {
    /// Checks if contract caller is an token owner
    fn check_token_owner(
        &self,
        contract_address: AccountId,
        token_id: Id,
    ) -> Result<(), MarketplaceError>;

    fn check_token_allowance(
        &self,
        contract_address: AccountId,
        token_id: Id,
    ) -> Result<(), MarketplaceError>;

    /// Checks token price.
    fn check_price(
        &self,
        transferred_value: Balance,
        price: Balance,
    ) -> Result<(), MarketplaceError>;

    /// Checks fee
    fn check_fee(&self, fee: u16, max_fee: u16) -> Result<(), MarketplaceError>;

    /// Checks if token is listed for sale on the marketplace.
    fn is_token_listed(&self, contract_address: AccountId, token_id: Id) -> bool;

    /// Transfers token.
    fn transfer_token(
        &self,
        contract_address: AccountId,
        token_id: Id,
        token_owner: AccountId,
        buyer: AccountId,
        seller_fee: Balance,
        marketplace_fee: Balance,
        royalty_receiver: AccountId,
        author_royalty: Balance,
        token_price: Balance,
    ) -> Result<(), MarketplaceError>;

    /// Get NFT contract hash needed for factory method
    fn get_nft_contract_hash(
        &self,
        contract_type: &NftContractType,
    ) -> Result<Hash, MarketplaceError>;

    fn get_deposit_internal(&self, account_id: AccountId) -> Balance;
}

pub trait MarketplaceSaleEvents {
    fn emit_token_listed_event(&self, contract: AccountId, token_id: Id, price: Option<Balance>);
    fn emit_token_bought_event(&self, contract: AccountId, token_id: Id, price: Balance);
    fn emit_make_offer_event(
        &self,
        bidder_id: AccountId,
        contract: AccountId,
        token_id: Option<Id>,
        quantity: u64,
        price_per_item: Balance,
        extra: String,
        offer_id: u128,
    );
    fn emit_collection_registered_event(&self, contract: AccountId);
}

impl<T> MarketplaceSale for T
where
    T: Storage<Data> + Storage<ownable::Data> + Storage<reentrancy_guard::Data>,
{
    /// Sets a hash of a Shiden34 contract to be instantiated by factory call.
    #[modifiers(only_owner)]
    default fn set_nft_contract_hash(
        &mut self,
        contract_type: NftContractType,
        contract_hash: Hash,
    ) -> Result<(), MarketplaceError> {
        self.data::<Data>()
            .nft_contract_hash
            .insert(&contract_type, &contract_hash);
        Ok(())
    }

    /// Gets a NFT contract hash.
    default fn nft_contract_hash(&self, contract_type: NftContractType) -> Hash {
        self.get_nft_contract_hash(&contract_type).unwrap()
    }

    /// Creates a NFT item sale on the marketplace.
    default fn list(
        &mut self,
        contract_address: AccountId,
        token_id: Id,
        price: Balance,
    ) -> Result<(), MarketplaceError> {
        self.check_token_owner(contract_address, token_id.clone())?;
        self.check_token_allowance(contract_address, token_id.clone())?;
        self.data::<Data>().items.insert(
            &(contract_address, token_id.clone()),
            &Item {
                owner: Self::env().caller(),
                price,
            },
        );
        self.emit_token_listed_event(contract_address, token_id, Some(price));
        Ok(())
    }

    /// Removes a NFT from the marketplace sale.
    default fn unlist(
        &mut self,
        contract_address: AccountId,
        token_id: Id,
    ) -> Result<(), MarketplaceError> {
        ensure!(
            self.is_token_listed(contract_address, token_id.clone()),
            MarketplaceError::ItemNotListedForSale
        );
        self.check_token_owner(contract_address, token_id.clone())?;

        self.data::<Data>()
            .items
            .remove(&(contract_address, token_id.clone()));
        self.emit_token_listed_event(contract_address, token_id, None);
        Ok(())
    }

    /// Buys NFT item from the marketplace.
    #[modifiers(non_reentrant)]
    default fn buy(
        &mut self,
        contract_address: AccountId,
        token_id: Id,
    ) -> Result<(), MarketplaceError> {
        let item = self
            .data::<Data>()
            .items
            .get(&(contract_address, token_id.clone()))
            .ok_or(MarketplaceError::ItemNotListedForSale)?;

        let token_owner = PSP34Ref::owner_of(&contract_address, token_id.clone())
            .ok_or(MarketplaceError::TokenDoesNotExist)?;
        let caller = Self::env().caller();
        ensure!(token_owner != caller, MarketplaceError::AlreadyOwner);

        let value = Self::env().transferred_value();
        self.check_price(value, item.price)?;

        let collection = self
            .data::<Data>()
            .registered_collections
            .get(&contract_address)
            .ok_or(MarketplaceError::NotRegisteredContract)?;

        let marketplace_fee = value
            .checked_mul(self.data::<Data>().fee as u128)
            .unwrap_or_default()
            / 10_000;
        let author_royalty = value
            .checked_mul(collection.royalty as u128)
            .unwrap_or_default()
            / 10_000;
        let seller_fee = value
            .checked_sub(marketplace_fee)
            .unwrap_or_default()
            .checked_sub(author_royalty)
            .unwrap_or_default();

        self.transfer_token(
            contract_address,
            token_id,
            token_owner,
            caller,
            seller_fee,
            marketplace_fee,
            collection.royalty_receiver,
            author_royalty,
            value,
        )
    }

    /// Registers NFT collection to the marketplace.
    default fn register(
        &mut self,
        contract_address: AccountId,
        royalty_receiver: AccountId,
        royalty: u16,
        marketplace_ipfs: String,
    ) -> Result<(), MarketplaceError> {
        let max_fee = self.data::<Data>().max_fee;
        self.check_fee(royalty, max_fee)?;

        let caller = Self::env().caller();

        // Check if caller is Marketplace owner of NFT owner.
        if self.data::<ownable::Data>().owner != caller
            && OwnableRef::owner(&contract_address) != caller
        {
            return Err(MarketplaceError::NotOwner);
        }

        if self
            .data::<Data>()
            .registered_collections
            .get(&contract_address)
            .is_some()
        {
            Err(MarketplaceError::ContractAlreadyRegistered)
        } else {
            self.data::<Data>().registered_collections.insert(
                &contract_address,
                &RegisteredCollection {
                    royalty_receiver,
                    royalty,
                    marketplace_ipfs,
                },
            );
            self.emit_collection_registered_event(contract_address);
            Ok(())
        }
    }

    /// Gets registered collection.
    default fn get_registered_collection(
        &self,
        contract_address: AccountId,
    ) -> Option<RegisteredCollection> {
        self.data::<Data>()
            .registered_collections
            .get(&contract_address)
    }

    /// Sets the marketplace fee.
    #[modifiers(only_owner)]
    default fn set_marketplace_fee(&mut self, fee: u16) -> Result<(), MarketplaceError> {
        let max_fee = self.data::<Data>().max_fee;
        self.check_fee(fee, max_fee)?;
        self.data::<Data>().fee = fee;

        Ok(())
    }

    /// Gets the marketplace fee.
    default fn get_marketplace_fee(&self) -> u16 {
        self.data::<Data>().fee
    }

    /// Gets max fee that can be applied to an item price.
    default fn get_max_fee(&self) -> u16 {
        self.data::<Data>().max_fee
    }

    /// Checks if NFT token is listed on the marketplace and returns token price.
    default fn get_price(&self, contract_address: AccountId, token_id: Id) -> Option<Balance> {
        match self.data::<Data>().items.get(&(contract_address, token_id)) {
            Some(item) => Some(item.price),
            _ => None,
        }
    }

    /// Sets contract metadata (ipfs url)
    #[modifiers(only_owner)]
    default fn set_contract_metadata(
        &mut self,
        contract_address: AccountId,
        ipfs: String,
    ) -> Result<(), MarketplaceError> {
        let collection = self
            .data::<Data>()
            .registered_collections
            .get(&contract_address)
            .ok_or(MarketplaceError::NotRegisteredContract)?;

        self.data::<Data>().registered_collections.insert(
            &contract_address,
            &RegisteredCollection {
                royalty_receiver: collection.royalty_receiver,
                marketplace_ipfs: ipfs,
                royalty: collection.royalty,
            },
        );

        Ok(())
    }

    /// Gets the marketplace fee recipient.
    default fn get_fee_recipient(&self) -> AccountId {
        self.data::<Data>().market_fee_recipient.unwrap()
    }

    /// Sets the marketplace fee recipient.
    #[modifiers(only_owner)]
    default fn set_fee_recipient(
        &mut self,
        fee_recipient: AccountId,
    ) -> Result<(), MarketplaceError> {
        self.data::<Data>().market_fee_recipient = Option::Some(fee_recipient);

        Ok(())
    }

    default fn deposit(&mut self) -> Result<(), MarketplaceError> {
        let caller = Self::env().caller();
        let value = Self::env().transferred_value();

        let current_balance = self.data::<Data>().deposit.get(&caller).unwrap_or(0);
        self.data::<Data>()
            .deposit
            .insert(&caller, &(value + current_balance));
        Ok(())
    }

    default fn withdraw(&mut self, amount: Balance) -> Result<(), MarketplaceError> {
        let caller = Self::env().caller();
        let current_balance = self.data::<Data>().deposit.get(&caller).unwrap_or(0);

        if current_balance < amount {
            return Err(MarketplaceError::BalanceInsufficient);
        } else {
            self.data::<Data>()
                .deposit
                .insert(&caller, &(current_balance - amount));
            Self::env()
                .transfer(caller, amount)
                .map_err(|_| MarketplaceError::TransferToOwnerFailed)?;
            Ok(())
        }
    }

    default fn get_deposit(&self, account_id: AccountId) -> Balance {
        self.get_deposit_internal(account_id)
    }

    default fn cancel_offer(&mut self, offer_id: u128) -> Result<(), MarketplaceError> {
        let caller = Self::env().caller();

        let offer = self.data::<Data>().offer_items.get(&offer_id).unwrap();

        if offer.bidder_id != caller {
            return Err(MarketplaceError::NotOwner);
        }

        self.data::<Data>().offer_items.remove(&offer_id);

        // TO DO: remove from enumerable

        Ok(())
    }

    default fn get_offer_active(&self, offer_id: u128) -> bool {
        let offer = self.data::<Data>().offer_items.get(&offer_id);

        if let Some(offer) = offer {
            let caller = Self::env().caller();
            let deposit = self.get_deposit_internal(caller);
            let total_amount = offer.quantity as u128 * offer.price_per_item;

            if deposit >= total_amount {
                return true;
            }
        }
        return false;
    }

    default fn accept_offer(
        &mut self,
        offer_id: u128,
        token_id: Id,
    ) -> Result<(), MarketplaceError> {
        let offer = self.data::<Data>().offer_items.get(&offer_id).unwrap();

        Ok(())
    }

    default fn fulfill_offer(
        &mut self,
        offer_id: u128,
        token_id: Id,
    ) -> Result<(), MarketplaceError> {
        Ok(())
    }

    default fn make_offer(
        &mut self,
        contract_address: AccountId,
        token_id: Option<Id>,
        quantity: u64,
        price_per_item: Balance,
        extra: String,
    ) -> Result<u128, MarketplaceError> {
        let caller = Self::env().caller();

        let total_amount = quantity as u128 * price_per_item;

        let deposit = self.get_deposit_internal(caller);

        if deposit < total_amount {
            return Err(MarketplaceError::BalanceInsufficient);
        }

        let current_offer_id = self.data::<Data>().last_offer_id + 1;

        self.data::<Data>().last_offer_id = current_offer_id;

        self.data::<Data>().offer_items.insert(
            &current_offer_id,
            &OfferItem {
                bidder_id: caller,
                contract_address,
                token_id: token_id.clone(),
                quantity,
                price_per_item,
                extra: extra.clone(),
            },
        );

        // TO DO: add to enumerable

        // Emit event
        self.emit_make_offer_event(
            caller,
            contract_address,
            token_id,
            quantity,
            price_per_item,
            extra,
            current_offer_id,
        );
        Ok(1)
    }
}

impl<T> MarketplaceSaleEvents for T
where
    T: Storage<Data>,
{
    default fn emit_token_listed_event(
        &self,
        _contract: AccountId,
        _token_id: Id,
        _price: Option<Balance>,
    ) {
    }

    default fn emit_token_bought_event(
        &self,
        _contract: AccountId,
        _token_id: Id,
        _price: Balance,
    ) {
    }

    default fn emit_collection_registered_event(&self, _contract: AccountId) {}

    default fn emit_make_offer_event(
        &self,
        _bidder_id: AccountId,
        _contract: AccountId,
        _token_id: Option<Id>,
        _quantity: u64,
        _price_per_item: u128,
        _extra: String,
        _offer_id: u128,
    ) {
    }
}

impl<T> Internal for T
where
    T: Storage<Data>,
{
    default fn check_token_owner(
        &self,
        contract_address: AccountId,
        token_id: Id,
    ) -> Result<(), MarketplaceError> {
        if !self
            .data::<Data>()
            .registered_collections
            .contains(&contract_address)
        {
            return Err(MarketplaceError::NotRegisteredContract);
        }

        let caller = Self::env().caller();
        match PSP34Ref::owner_of(&contract_address, token_id) {
            Some(token_owner) => {
                ensure!(caller == token_owner, MarketplaceError::NotOwner);
                Ok(())
            }
            None => Err(MarketplaceError::TokenDoesNotExist),
        }
    }

    default fn check_token_allowance(
        &self,
        contract_address: AccountId,
        token_id: Id,
    ) -> Result<(), MarketplaceError> {
        let caller = Self::env().caller();
        let current_contract_id = Self::env().account_id();
        match PSP34Ref::allowance(
            &contract_address,
            caller,
            current_contract_id,
            Some(token_id),
        ) {
            false => Err(MarketplaceError::TokenNotApproved),
            true => Ok(()),
        }
    }

    default fn check_price(
        &self,
        transferred_value: Balance,
        price: Balance,
    ) -> Result<(), MarketplaceError> {
        ensure!(transferred_value >= price, MarketplaceError::BadBuyValue);

        Ok(())
    }

    default fn check_fee(&self, fee: u16, max_fee: u16) -> Result<(), MarketplaceError> {
        ensure!(fee <= max_fee, MarketplaceError::FeeTooHigh);

        Ok(())
    }

    default fn is_token_listed(&self, contract_address: AccountId, token_id: Id) -> bool {
        self.data::<Data>()
            .items
            .get(&(contract_address, token_id))
            .is_some()
    }

    default fn transfer_token(
        &self,
        contract_address: AccountId,
        token_id: Id,
        token_owner: AccountId,
        buyer: AccountId,
        seller_fee: Balance,
        marketplace_fee: Balance,
        royalty_receiver: AccountId,
        author_royalty: Balance,
        token_price: Balance,
    ) -> Result<(), MarketplaceError> {
        match PSP34Ref::transfer(
            &contract_address,
            buyer,
            token_id.clone(),
            ink::prelude::vec::Vec::new(),
        ) {
            Ok(()) => {
                Self::env()
                    .transfer(token_owner, seller_fee)
                    .map_err(|_| MarketplaceError::TransferToOwnerFailed)?;
                Self::env()
                    .transfer(
                        self.data::<Data>().market_fee_recipient.unwrap(),
                        marketplace_fee,
                    )
                    .map_err(|_| MarketplaceError::TransferToMarketplaceFailed)?;
                Self::env()
                    .transfer(royalty_receiver, author_royalty)
                    .map_err(|_| MarketplaceError::TransferToAuthorFailed)?;
                self.emit_token_bought_event(contract_address, token_id, token_price);
                Ok(())
            }
            Err(_) => Err(MarketplaceError::UnableToTransferToken),
        }
    }

    default fn get_nft_contract_hash(
        &self,
        contract_type: &NftContractType,
    ) -> Result<Hash, MarketplaceError> {
        self.data::<Data>()
            .nft_contract_hash
            .get(&contract_type)
            .ok_or(MarketplaceError::NftContractHashNotSet)
    }

    default fn get_deposit_internal(&self, account_id: AccountId) -> Balance {
        self.data::<Data>().deposit.get(&account_id).unwrap_or(0)
    }
}
