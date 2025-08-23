use bevy::{math::DVec3, prelude::*};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::f64::consts::PI;

use crate::{
    orrery::{Celestial, Orrery},
    physics::{Velocity, WithinSoi, sim_time},
    precision::{PreciseTransform, ToMetersExt, ToMillimetersExt},
};

#[derive(Component, Default, Serialize, Deserialize)]
pub struct AeroEnv {
    pub planet: SmolStr,
    pub planet_rel: PreciseTransform,

    pub altitude: f64,
    pub pressure: f64,
    pub density: f64,
    pub temperature: f64,

    pub airspeed: DVec3,
}

pub(super) fn update_aero_env(
    orrery: Res<Orrery>,
    mut obj: Query<(&PreciseTransform, &Velocity, &WithinSoi, &mut AeroEnv)>,
    planets: Query<(&Celestial, &PreciseTransform)>,
    time: Res<Time>,
) {
    let epoch = sim_time(&time);
    obj.par_iter_mut()
        .for_each(|(ptf, velocity, soi, mut params)| {
            let (planet, planet_ptf) = planets.get(soi.0).unwrap();
            let body = orrery.get_body(&planet.0).unwrap();
            let rel_translation = ptf.translation_mm - planet_ptf.translation_mm;
            let r_vec = (ptf.translation_mm - planet_ptf.translation_mm).to_meters_64();
            let spin_period = body.rotation.rotation_period;
            // calculate the local atmospheric velocity
            let mut v_atm: DVec3;

            if spin_period > 0.0 {
                let spin_rate = 2.0 * PI / spin_period;
                let spin_axis = planet_ptf.rotation * DVec3::new(0.0, 0.0, 1.0);
                v_atm = spin_axis.cross(r_vec) * spin_rate;
            } else {
                v_atm = DVec3::ZERO;
            }
            if let Some(v_orb) = orrery.solve_velocity(&planet.0, epoch) {
                v_atm += v_orb;
            }
            // calculate the params
            params.altitude = r_vec.length() - body.radius;
            params.airspeed = velocity.0 - v_atm;
            let data = pannea_atm(params.altitude - 144_000.0);
            params.density = data.density;
            params.pressure = data.pressure;
            params.temperature = data.temperature;
            params.planet = planet.0.clone();
            let planet_rot_inverse = planet_ptf.rotation.inverse();
            params.planet_rel.translation_mm =
                (planet_rot_inverse * rel_translation.to_meters_64()).to_millimeters();
            params.planet_rel.rotation = planet_rot_inverse * ptf.rotation;
        });
}

#[derive(Debug, Clone, Copy)]
struct PanneaDatum {
    pub pressure: f64,    // Pa
    pub density: f64,     // kg m⁻³
    pub temperature: f64, // K
    pub opacity: f64,     // 0‥1  (fraction of sunlight transmitted)
}

/// Simple “standard-atmosphere” model for the sky-world **Pannea**.
///
/// * `altitude` — metres **above the 1-bar layer** (positive = higher, negative = deeper).  
///   The 1-bar datum is ≈ 285 K and ρ ≈ 1.39 kg m⁻³.
///
/// The model is piece-wise:
/// * Troposphere: linear lapse-rate **L = 6 K km⁻¹** down to the 1 GPa “death-zone”
///
/// * Tropopause: at **hₜ = 10 000 m** the temperature bottoms at **Tₜ = 225 K**  
///   Above this, an isothermal stratosphere (≈ 220 K) is assumed.
///
/// * Pressure in the lapse region uses the standard ideal-gas/hydrostatic relation;  
///   above the tropopause, pressure decays exponentially with scale-height
///   **H_iso = Rᵣₛ·T_iso / g ≈ 5 500 m**.
///
/// * “Opacity” is a toy optical-depth model:
///   `opacity = exp( −k · P )` with *k* = 1.5 × 10⁻⁵ Pa⁻¹.  
///   → About **22 %** of solar flux reaches the 1-bar deck (matching the
///   “bright overcast” description) and ≳80 % reaches the top of broken-cloud
///   layers at ~0.4 bar.
///
/// > **Caveat** Real weather on Pannea varies ±20 K and ±30 % pressure inside
/// > cyclones; this routine is only a background reference.
fn pannea_atm(altitude: f64) -> PanneaDatum {
    // ---------- constants ----------
    const G: f64 = 10.0; // m s⁻²  (surface gravity)
    const R_UNIV: f64 = 8.314_462_618; // J mol⁻¹ K⁻¹
    const MU: f64 = 0.033; // kg mol⁻¹  (mean mol. mass 33 g)
    const R_SPEC: f64 = R_UNIV / MU; // J kg⁻¹ K⁻¹ ≈ 252
    const T0: f64 = 285.0; // K  (1-bar layer)
    const P0: f64 = 1.0e5; // Pa
    const LAPSE: f64 = 0.006; // K m⁻¹ (6 K km⁻¹)
    const HTROP: f64 = 10_000.0; // m  (tropopause above 1 bar)
    const T_TROP: f64 = 225.0; // K  bottom-out temperature
    const T_ISO: f64 = 220.0; // K  isothermal stratosphere
    const K_OPA: f64 = 1.5e-5; // Pa⁻¹  (opacity coefficient)

    // exponent used in the Poisson formula (g / (R*L))
    const EXPONENT: f64 = G / (R_SPEC * LAPSE);

    // ---------- temperature profile ----------
    let (temp, pressure) = if altitude <= HTROP {
        // Linear lapse region (handles negative altitude too)
        let t = T0 - LAPSE * altitude;
        let t_clamped = t.max(150.0); // keep numeric sanity deep down
        let p = P0 * (t_clamped / T0).powf(EXPONENT);
        (t_clamped, p)
    } else {
        // Isothermal upper layer
        // First: conditions at tropopause
        let p_trop = P0 * (T_TROP / T0).powf(EXPONENT);
        let h = altitude - HTROP;
        let h_scale = (R_SPEC * T_ISO) / G; // ≈ 5.5 km
        let p = p_trop * (-h / h_scale).exp();
        (T_ISO, p)
    };

    // ---------- density ----------
    let density = pressure / (R_SPEC * temp);

    // ---------- toy optical-depth / opacity ----------
    let opacity = (-K_OPA * pressure).exp().clamp(0.0, 1.0);

    PanneaDatum {
        pressure,
        density,
        temperature: temp,
        opacity,
    }
}
