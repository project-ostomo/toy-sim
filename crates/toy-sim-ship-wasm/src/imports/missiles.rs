use super::*;

fn observation(host: &Host) -> CallResult<w::MissileObservation> {
    let Some(CallbackKind::Missile(handle)) = host.callback else {
        return Err(w::ERR_UNAVAILABLE.into());
    };
    host.missile
        .filter(|observation| observation.handle == handle)
        .ok_or_else(|| w::ERR_UNAVAILABLE.into())
}

pub(super) fn register(linker: &mut Linker<Host>) -> Result<()> {
    metered!(
        linker,
        "missile_read",
        |mut caller: Caller<'_, Host>, pointer: u32, bytes: u32| {
            CallPlan::record::<w::MissileObservation>(&caller, pointer, bytes)
        },
        {
            status((|| {
                let observation = observation(caller.data())?;
                emit(&mut caller, pointer, bytes, &observation)
            })())
        },
    )?;

    metered!(
        linker,
        "missile_control",
        |mut caller: Caller<'_, Host>, pointer: u32, bytes: u32| {
            CallPlan::record::<w::MissileControl>(&caller, pointer, bytes)
        },
        {
            status((|| {
                let observation = observation(caller.data())?;
                let control: w::MissileControl = input(&mut caller, pointer, bytes)?;
                finite(&control.direction)?;
                fraction(control.throttle)?;
                let length_squared = control
                    .direction
                    .iter()
                    .map(|value| value * value)
                    .sum::<f64>();
                let is_unit = (length_squared - 1.).abs() <= 1e-6;
                if !is_unit && !(control.throttle == 0. && length_squared == 0.) {
                    return Err(w::ERR_ARGUMENT.into());
                }
                let controls = &mut caller.data_mut().output.missiles;
                controls.clear();
                controls.push((observation.handle, control));
                Ok(())
            })())
        },
    )?;
    Ok(())
}
