use super::*;

#[derive(Clone, Debug)]
pub struct ControllerCheckpoint {
    pub program: Vec<u8>,
    pub persistent_data: Vec<u8>,
}

impl ControllerCheckpoint {
    pub fn install_chatter_profile(
        &mut self,
        profile: osg_model::firmware::ChatterProfile,
    ) -> Result<()> {
        use osg_model::firmware::ProgramMemory;

        ensure!(profile.valid(), "invalid chatter profile");
        let mut memory: ProgramMemory = if self.persistent_data.is_empty() {
            ProgramMemory::default()
        } else {
            postcard::from_bytes(&self.persistent_data)
                .context("program memory is not a chatter envelope")?
        };
        memory.chatter = Some(profile);
        let encoded = postcard::to_stdvec(&memory)?;
        ensure!(encoded.len() <= 65536, "program memory exceeds 64 KiB");
        self.persistent_data = encoded;
        Ok(())
    }
}

impl Controller {
    pub fn checkpoint(&self) -> ControllerCheckpoint {
        ControllerCheckpoint {
            program: self.program.to_vec(),
            persistent_data: self.persistent_data.clone(),
        }
    }
}

impl ControllerRuntime {
    pub fn instantiate_with_chatter(
        &mut self,
        program: &[u8],
        profile: osg_model::firmware::ChatterProfile,
    ) -> Result<Controller> {
        let mut checkpoint = ControllerCheckpoint {
            program: program.to_vec(),
            persistent_data: Vec::new(),
        };
        checkpoint.install_chatter_profile(profile)?;
        self.restore(&checkpoint)
    }

    pub fn restore(&mut self, checkpoint: &ControllerCheckpoint) -> Result<Controller> {
        ensure!(
            checkpoint.persistent_data.len() <= 65536,
            "persistent program data exceeds 64 KiB"
        );
        let mut controller = self.instantiate(&checkpoint.program)?;
        controller
            .persistent_data
            .clone_from(&checkpoint.persistent_data);
        Ok(controller)
    }
}
