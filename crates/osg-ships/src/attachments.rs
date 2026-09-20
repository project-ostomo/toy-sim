use crate::{Catalogue, Equipment, GRID, PartDef, PlacedPart, PreparedPart, ShipBlueprint};
use anyhow::{Context, Result, ensure};
use glam::{DMat3, DVec3};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub parent: u64,
    pub socket: String,
    pub plug: String,
    pub roll: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AttachmentNode {
    pub name: String,
    pub connector: String,
    pub position_m: [f64; 3],
    pub normal: [f64; 3],
}

impl AttachmentNode {
    pub fn frame(&self) -> DMat3 {
        let z = DVec3::from_array(self.normal);
        let up = if z.y.abs() > 0.9 { DVec3::Z } else { DVec3::Y };
        let x = up.cross(z).normalize();
        DMat3::from_cols(x, z.cross(x), z)
    }
}

impl PartDef {
    pub fn attachment_nodes(&self) -> Vec<AttachmentNode> {
        if !self.nodes.is_empty() {
            return self.nodes.clone();
        }
        let half = DVec3::from_array(self.dimensions.map(|n| n as f64 * GRID)) * 0.5;
        let station = matches!(
            self.equipment,
            Equipment::Utility {
                utility: crate::utilities::UtilityDef::DirectoryTransmitter { .. }
                    | crate::utilities::UtilityDef::Docking { .. }
            }
        );
        let mut nodes = Vec::new();
        for (name, normal) in [
            ("front", DVec3::NEG_Z),
            ("back", DVec3::Z),
            ("left", DVec3::NEG_X),
            ("right", DVec3::X),
            ("bottom", DVec3::NEG_Y),
            ("top", DVec3::Y),
        ] {
            nodes.push(AttachmentNode {
                name: name.into(),
                connector: if station {
                    "station_backbone"
                } else {
                    "equipment"
                }
                .into(),
                position_m: (half * normal).to_array(),
                normal: normal.to_array(),
            });
        }
        if matches!(
            self.equipment,
            Equipment::Structure
                | Equipment::Engine { .. }
                | Equipment::ThermalEngine { .. }
                | Equipment::MicropulseEngine { .. }
        ) {
            for (name, normal) in [("fore", DVec3::NEG_Z), ("aft", DVec3::Z)] {
                nodes.push(AttachmentNode {
                    name: name.into(),
                    connector: format!("hull_{}", self.dimensions[0].min(self.dimensions[1])),
                    position_m: (half * normal).to_array(),
                    normal: normal.to_array(),
                });
            }
        }
        nodes
    }
}

impl ShipBlueprint {
    pub fn layout(&self, cat: &Catalogue) -> Result<Vec<PreparedPart>> {
        if self.parts.is_empty() {
            return Ok(Vec::new());
        }
        ensure!(self.parts.len() <= 4096, "too many parts");
        let mut remaining = BTreeMap::new();
        for part in &self.parts {
            ensure!(
                remaining.insert(part.id, part).is_none(),
                "duplicate part ID"
            );
        }
        ensure!(
            self.parts.iter().filter(|p| p.attachment.is_none()).count() == 1,
            "assembly needs one root part"
        );
        let mut resolved: BTreeMap<u64, PreparedPart> = BTreeMap::new();
        let mut occupied = BTreeSet::new();
        while !remaining.is_empty() {
            let mut progressed = false;
            for (&id, part) in remaining.clone().iter() {
                let def = cat.part(&part.prototype).context("unknown part")?;
                let (centre, rotation) = if let Some(mount) = &part.attachment {
                    let Some(parent) = resolved.get(&mount.parent) else {
                        continue;
                    };
                    ensure!(mount.roll < 4, "invalid attachment roll");
                    let parent_nodes = parent.definition.attachment_nodes();
                    let child_nodes = def.attachment_nodes();
                    let socket = parent_nodes
                        .iter()
                        .find(|n| n.name == mount.socket)
                        .context("unknown socket")?;
                    let plug = child_nodes
                        .iter()
                        .find(|n| n.name == mount.plug)
                        .context("unknown plug")?;
                    ensure!(
                        socket.connector == plug.connector,
                        "incompatible connectors: {} / {}",
                        socket.connector,
                        plug.connector
                    );
                    ensure!(
                        occupied.insert((mount.parent, mount.socket.clone())),
                        "socket already occupied"
                    );
                    ensure!(
                        occupied.insert((id, mount.plug.clone())),
                        "plug already occupied"
                    );
                    let flip = DMat3::from_rotation_y(std::f64::consts::PI);
                    let roll =
                        DMat3::from_rotation_z(mount.roll as f64 * std::f64::consts::FRAC_PI_2);
                    let rotation =
                        parent.rotation * socket.frame() * roll * flip * plug.frame().transpose();
                    let centre = parent.centre
                        + parent.rotation * DVec3::from_array(socket.position_m)
                        - rotation * DVec3::from_array(plug.position_m);
                    (centre, rotation)
                } else {
                    (DVec3::ZERO, DMat3::IDENTITY)
                };
                resolved.insert(
                    id,
                    PreparedPart {
                        placed: (*part).clone(),
                        definition: def.clone(),
                        centre,
                        rotation,
                    },
                );
                remaining.remove(&id);
                progressed = true;
            }
            ensure!(progressed, "attachment graph has a cycle or missing parent");
        }
        Ok(self
            .parts
            .iter()
            .map(|p| resolved.remove(&p.id).unwrap())
            .collect())
    }

    pub fn attach(
        &mut self,
        prototype: &str,
        parent: u64,
        socket: &str,
        plug: &str,
        roll: u8,
    ) -> u64 {
        let id = self.parts.iter().map(|p| p.id).max().unwrap_or(0) + 1;
        self.parts.push(PlacedPart {
            id,
            name: String::new(),
            alias: String::new(),
            groups: vec![],
            tanks: vec![],
            prototype: prototype.into(),
            attachment: if self.parts.is_empty() {
                None
            } else {
                Some(Attachment {
                    parent,
                    socket: socket.into(),
                    plug: plug.into(),
                    roll,
                })
            },
        });
        id
    }
}

impl PreparedPart {
    pub fn bounds(&self) -> (DVec3, DVec3) {
        let half = self.rotation.abs()
            * DVec3::from_array(self.definition.dimensions.map(|n| n as f64 * GRID))
            * 0.5;
        (self.centre - half, self.centre + half)
    }
}
