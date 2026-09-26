use super::*;

#[derive(Clone, Debug)]
pub struct ControllerCheckpoint {
    pub program: Vec<u8>,
    pub persistent_data: Vec<u8>,
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
    pub fn restore(&mut self, checkpoint: &ControllerCheckpoint) -> Result<Controller> {
        let mut controller = self.instantiate(&checkpoint.program)?;
        controller
            .persistent_data
            .clone_from(&checkpoint.persistent_data);
        Ok(controller)
    }
}
