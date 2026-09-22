use super::*;

pub fn validate_request(request: &osg_model::routing::Request) -> Result<()> {
    ensure!(request.id != 0, "invalid route request id");
    ensure!(request.preferences.valid(), "invalid planning preference");
    ensure!(
        request.orders.len() <= osg_model::routing::MAX_ORDERS,
        "too many requested waypoints"
    );
    for order in &request.orders {
        validate_order(order)?;
    }
    Ok(())
}
