use crate::SensorContact;
use crate::spatial::{Snapshot, State};
use std::{collections::BTreeMap, sync::Arc};
use toy_sim_ship_api::abi;

/// Committed host state. Each callback edits a private working copy.
#[derive(Clone, Default)]
pub struct Session {
    pub serial: toy_sim_model::serial::Terminal,
    pub spatial: State,
    pub attitude: Option<abi::AttitudeState>,
    pub weapons: Option<abi::WeaponsState>,
    pub weapon_rows: Vec<abi::WeaponInstrument>,
    pub navigation: Option<abi::NavigationState>,
    pub contacts: Option<abi::ContactsState>,
    pub contact_list: Arc<[SensorContact]>,
    pub contact_epoch: Option<f64>,
    pub(crate) pins: BTreeMap<u64, Snapshot>,
    pub(crate) latest_scan: Arc<[SensorContact]>,
    pub(crate) latest_scan_epoch: Option<f64>,
    pub(crate) latest_sensor: u64,
    pub(crate) screens: Vec<abi::ScreenDefinition>,
}

impl Session {
    pub fn expire(&mut self, time: f64) {
        self.spatial.expire(time);
        if self
            .weapons
            .is_some_and(|state| state.valid_until_s <= time)
        {
            self.weapons = None;
            self.weapon_rows.clear();
        }

        if self
            .attitude
            .is_some_and(|state| state.valid_until_s <= time)
        {
            self.attitude = None;
        }

        if self
            .navigation
            .is_some_and(|state| state.valid_until_s <= time)
        {
            self.navigation = None;
        }

        if self
            .contacts
            .is_some_and(|state| state.valid_until_s <= time)
        {
            self.contacts = None;
            self.contact_list = Arc::default();
            self.contact_epoch = None;
        }
    }

    pub(crate) fn snapshot(
        &self,
        id: u64,
        current: Snapshot,
        borrowed: Option<Snapshot>,
    ) -> Option<Snapshot> {
        if id == current.id {
            Some(current)
        } else {
            borrowed
                .filter(|snapshot| snapshot.id == id)
                .or_else(|| self.pins.get(&id).copied())
        }
    }
}
