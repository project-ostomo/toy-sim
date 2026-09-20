pub fn distance(meters: f64) -> String {
    const LIGHT_YEAR_M: f64 = 9_460_730_472_580_800.;

    if meters.abs() >= LIGHT_YEAR_M * 0.1 {
        format!("{:.2} ly", meters / LIGHT_YEAR_M)
    } else if meters.abs() >= 1e12 {
        format!("{:.2} AU", meters / 149_597_870_700.)
    } else if meters.abs() >= 1e9 {
        format!("{:.2} Gm", meters / 1e9)
    } else if meters.abs() >= 1e6 {
        format!("{:.2} Mm", meters / 1e6)
    } else if meters.abs() >= 1e3 {
        format!("{:.2} km", meters / 1e3)
    } else {
        format!("{meters:.0} m")
    }
}

pub fn mass(kg: f64) -> String {
    if kg.abs() >= 1000. {
        format!("{:.1} t", kg / 1000.)
    } else {
        format!("{kg:.1} kg")
    }
}
