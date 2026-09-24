use super::*;

pub fn validate_request(request: &osg_model::routing::Request) -> Result<()> {
    ensure!(request.id != 0, "invalid route request id");
    ensure!(request.preferences.valid(), "invalid planning preference");
    ensure!(
        request.directives.len() <= osg_model::routing::MAX_DIRECTIVES,
        "too many requested directives"
    );
    Ok(())
}
