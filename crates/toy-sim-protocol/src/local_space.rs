use super::*;
use toy_sim_model::local_space::{MAX_LOCAL_OBSTACLES, MAX_RANGE_M};

pub fn validate_request(
    destination: GalacticPosition,
    range_m: f64,
    after_seconds: f64,
) -> Result<()> {
    ensure!(
        position_valid(destination),
        "invalid local-space destination"
    );
    ensure!(
        range_m.is_finite() && (0.0..=MAX_RANGE_M).contains(&range_m),
        "invalid local-space range"
    );
    ensure!(
        after_seconds.is_finite()
            && (0.0..=travel::MAX_PREDICTION_SECONDS).contains(&after_seconds),
        "invalid local-space prediction time"
    );
    Ok(())
}

pub fn validate_reply(reply: &LocalSpace) -> Result<()> {
    ensure!(
        reply.obstacles.len() <= MAX_LOCAL_OBSTACLES,
        "too many local obstacles"
    );
    for obstacle in &reply.obstacles {
        ensure!(pose_valid(&obstacle.pose), "invalid local obstacle pose");
        ensure!(
            [obstacle.radius_m, obstacle.slip_exclusion_m]
                .into_iter()
                .all(|radius| radius.is_finite() && radius >= 0.0)
                && (obstacle.radius_m > 0.0 || obstacle.slip_exclusion_m > 0.0),
            "invalid local obstacle radius"
        );
        match &obstacle.reference {
            travel::Target::Contact(_) => {}
            travel::Target::Destination(
                destination @ (travel::Destination::Beacon(_)
                | travel::Destination::Relative { .. }),
            ) => validate_destination(destination)?,
            _ => bail!("local obstacle requires a public or observed reference"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_observation_limits_and_reference_contract_are_validated() {
        validate_request(GalacticPosition::ZERO, 0.0, 0.0).unwrap();
        validate_request(
            GalacticPosition::ZERO,
            MAX_RANGE_M,
            travel::MAX_PREDICTION_SECONDS,
        )
        .unwrap();
        for invalid in [f64::NAN, f64::INFINITY, -1.0, MAX_RANGE_M + 1.0] {
            assert!(validate_request(GalacticPosition::ZERO, invalid, 0.0).is_err());
        }
        assert!(validate_request(GalacticPosition::ZERO, 1.0, f64::NAN).is_err());

        let obstacle = LocalObstacle {
            reference: travel::Target::Contact(ContactRef {
                group: Id::new(),
                track: Id::new(),
            }),
            pose: Pose::default(),
            radius_m: 10.0,
            slip_exclusion_m: 0.0,
        };
        let mut reply = LocalSpace {
            obstacles: vec![obstacle; MAX_LOCAL_OBSTACLES],
            truncated: true,
        };
        validate_reply(&reply).unwrap();
        let bytes = postcard::to_allocvec(&ProgramReply::LocalSpace(reply.clone())).unwrap();
        assert!(bytes.len() < 65536);
        reply.obstacles.push(reply.obstacles[0].clone());
        assert!(validate_reply(&reply).is_err());
        reply.obstacles.truncate(1);
        reply.obstacles[0].reference = travel::Target::Direction([1., 0., 0.]);
        assert!(validate_reply(&reply).is_err());
        reply.obstacles[0].reference =
            travel::Target::Destination(travel::Destination::Beacon(Id::new()));
        reply.obstacles[0].radius_m = f64::NAN;
        assert!(validate_reply(&reply).is_err());
    }
}
