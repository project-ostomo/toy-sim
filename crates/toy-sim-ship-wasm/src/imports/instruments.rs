use super::*;

pub(super) fn register(linker: &mut Linker<Host>) -> Result<()> {
    linker.func_wrap(
        w::IMPORT_MODULE,
        "instrument_weapons_put",
        |mut caller: Caller<'_, Host>, pointer: u32, bytes: u32, rows_pointer: u32, count: u32| {
            status((|| {
                enter(&mut caller)?;
                let value: w::WeaponsState = input(&mut caller, pointer, bytes)?;
                lease(&caller, value.valid_until_s)?;
                if value.mode > w::WEAPONS_ENGAGE
                    || value.reason.as_str().is_none()
                    || count as usize > caller.data().catalogue.len()
                {
                    return Err(w::ERR_ARGUMENT);
                }
                let length = count
                    .checked_mul(size_of::<w::WeaponInstrument>() as u32)
                    .ok_or(w::ERR_BUFFER)?;
                if range(&caller, rows_pointer, length).is_none() {
                    return Err(w::ERR_BUFFER);
                }
                pay(
                    &mut caller,
                    u64::from(count) * w::WEAPON_ROW_GAS + u64::from(length) / 8,
                )?;
                let mut rows = Vec::with_capacity(count as usize);
                let mut ids = std::collections::BTreeSet::new();
                for index in 0..count {
                    let row: w::WeaponInstrument = read(
                        &caller,
                        rows_pointer + index * size_of::<w::WeaponInstrument>() as u32,
                        size_of::<w::WeaponInstrument>() as u32,
                    )
                    .ok_or(w::ERR_BUFFER)?;
                    let device = device_index(&caller, row.device)?;
                    if caller.data().catalogue[device].kind.abi_tag() != w::DEVICE_WEAPON
                        || !ids.insert(row.device)
                        || row.solution_flags > w::WEAPON_SOLUTION
                    {
                        return Err(w::ERR_ARGUMENT);
                    }
                    finite(&[
                        row.time_of_flight_s,
                        row.pointing_error_rad,
                        row.reading.battery_energy_j,
                        row.reading.shot_energy_j,
                        row.reading.yaw_rad,
                        row.reading.pitch_rad,
                        row.reading.next_fire_s,
                    ])?;
                    if row.time_of_flight_s < 0.0 || row.pointing_error_rad < 0.0 {
                        return Err(w::ERR_ARGUMENT);
                    }
                    if row.aim_marker != 0
                        && !caller
                            .data()
                            .working
                            .spatial
                            .markers
                            .contains_key(&row.aim_marker)
                    {
                        return Err(w::ERR_HANDLE);
                    }
                    rows.push(row);
                }
                let state = &mut caller.data_mut().working;
                state.weapons = Some(value);
                state.weapon_rows = rows;
                Ok(())
            })())
        },
    )?;

    linker.func_wrap(
        w::IMPORT_MODULE,
        "instrument_attitude_put",
        |mut caller: Caller<'_, Host>, pointer: u32, bytes: u32| {
            status((|| {
                enter(&mut caller)?;
                let value: w::AttitudeState = input(&mut caller, pointer, bytes)?;
                lease(&caller, value.valid_until_s)?;
                finite(&value.reference)?;
                finite(&[value.control_error])?;

                if value.mode > w::ATTITUDE_GUIDANCE
                    || value.present > w::ATTITUDE_REFERENCE
                    || value.control_error < 0.
                {
                    return Err(w::ERR_ARGUMENT);
                }

                if value.present == 0 {
                    if value.reference != [0.; 4] {
                        return Err(w::ERR_ARGUMENT);
                    }
                } else if (value
                    .reference
                    .iter()
                    .map(|value| value * value)
                    .sum::<f64>()
                    - 1.)
                    .abs()
                    > 1e-6
                {
                    return Err(w::ERR_ARGUMENT);
                }

                caller.data_mut().working.attitude = Some(value);
                Ok(())
            })())
        },
    )?;

    linker.func_wrap(
        w::IMPORT_MODULE,
        "instrument_navigation_put",
        |mut caller: Caller<'_, Host>, pointer: u32, bytes: u32| {
            status((|| {
                enter(&mut caller)?;
                let value: w::NavigationState = input(&mut caller, pointer, bytes)?;
                lease(&caller, value.valid_until_s)?;
                fraction(value.throttle)?;
                fraction(value.throttle_limit)?;

                if value.status > w::NAV_UNAVAILABLE
                    || value.present & !31 != 0
                    || value.reason.as_str().is_none()
                {
                    return Err(w::ERR_ARGUMENT);
                }

                for (bit, measurement) in [
                    (w::NAV_STAND_OFF, value.stand_off_m),
                    (w::NAV_SPEED_LIMIT, value.approach_speed_limit_m_s),
                    (w::NAV_BRAKING_DISTANCE, value.braking_distance_m),
                    (w::NAV_ARRIVAL, value.arrival_time_s),
                    (w::NAV_FUEL, value.predicted_fuel_kg),
                ] {
                    if !measurement.is_finite()
                        || measurement < 0.
                        || (value.present & bit == 0 && measurement != 0.)
                    {
                        return Err(w::ERR_ARGUMENT);
                    }
                }

                caller.data_mut().working.navigation = Some(value);
                Ok(())
            })())
        },
    )?;

    linker.func_wrap(
        w::IMPORT_MODULE,
        "instrument_contacts_put",
        |mut caller: Caller<'_, Host>, pointer: u32, bytes: u32| {
            status((|| {
                enter(&mut caller)?;
                let value: w::ContactsState = input(&mut caller, pointer, bytes)?;
                lease(&caller, value.valid_until_s)?;
                let host = caller.data_mut();

                if host
                    .working
                    .latest_scan_epoch
                    .is_none_or(|time| time + 2. <= host.current.epoch)
                {
                    return Err(w::ERR_UNAVAILABLE);
                }

                host.working.contacts = Some(value);
                host.working.contact_list = host.working.latest_scan.clone();
                host.working.contact_epoch = host.working.latest_scan_epoch;
                Ok(())
            })())
        },
    )?;

    linker.func_wrap(
        w::IMPORT_MODULE,
        "instrument_clear",
        |mut caller: Caller<'_, Host>, kind: u64| {
            status((|| {
                enter(&mut caller)?;
                let state = &mut caller.data_mut().working;

                match kind {
                    w::INSTRUMENT_WEAPONS => {
                        state.weapons = None;
                        state.weapon_rows.clear();
                    }
                    w::INSTRUMENT_ATTITUDE => state.attitude = None,
                    w::INSTRUMENT_NAVIGATION => state.navigation = None,
                    w::INSTRUMENT_CONTACTS => {
                        state.contacts = None;
                        state.contact_list = Arc::default();
                        state.contact_epoch = None;
                    }
                    _ => return Err(w::ERR_ARGUMENT),
                }

                Ok(())
            })())
        },
    )?;

    Ok(())
}
