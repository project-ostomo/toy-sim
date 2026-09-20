use anyhow::{Result, ensure};
use wasm_encoder::{
    reencode::{self, Reencode},
    *,
};
use wasmparser::{Operator, Parser, Payload, TypeRef};

pub(crate) const MODULE: &str = "__private_meter";
pub(crate) const ADMIT: &str = "admit";
pub(crate) const REMAINING_EXPORT: &str = "__private_remaining";
const MAX_SEGMENT_OPERATORS: usize = 64;
const MEMORY_BYTES_PER_GAS: i64 = 64;
const PAGE_GAS: i64 = 65_536 / MEMORY_BYTES_PER_GAS;
type RewriteResult<T = ()> = std::result::Result<T, reencode::Error<String>>;

enum BulkCost {
    Bytes,
    Pages,
    Elements,
}

fn supported_features() -> wasmparser::WasmFeatures {
    use wasmparser::WasmFeatures as Features;

    Features::MUTABLE_GLOBAL
        | Features::FLOATS
        | Features::SATURATING_FLOAT_TO_INT
        | Features::SIGN_EXTENSION
        | Features::REFERENCE_TYPES
        | Features::MULTI_VALUE
        | Features::BULK_MEMORY
        | Features::TAIL_CALL
        | Features::EXTENDED_CONST
}

pub(crate) fn instrument(bytes: &[u8]) -> Result<Vec<u8>> {
    let features = supported_features();
    // Validate before adding indices: the original program must not be able to
    // name the new budget global, admission import, or charging function.
    wasmparser::Validator::new_with_features(features).validate_all(bytes)?;

    let mut pass = Meter::default();
    for payload in Parser::new(0).parse_all(bytes) {
        match payload? {
            Payload::TypeSection(types) => {
                for ty in types.into_iter_err_on_gc_types() {
                    pass.params.push(ty?.params().len() as u32);
                }
            }
            Payload::ImportSection(imports) => {
                for import in imports.into_imports() {
                    let import = import?;
                    ensure!(import.module != MODULE, "private metering import reserved");
                    match import.ty {
                        TypeRef::Func(_) => pass.imported_functions += 1,
                        TypeRef::Global(_) => pass.globals += 1,
                        _ => {}
                    }
                }
            }
            Payload::FunctionSection(functions) => {
                pass.functions = functions
                    .into_iter()
                    .collect::<std::result::Result<_, _>>()?;
            }
            Payload::GlobalSection(globals) => pass.globals += globals.count(),
            Payload::ExportSection(exports) => {
                for export in exports {
                    ensure!(
                        export?.name != REMAINING_EXPORT,
                        "private metering export reserved"
                    );
                }
            }
            _ => {}
        }
    }
    ensure!(
        !pass.functions.is_empty(),
        "controller requires a defined function"
    );

    pass.helper = pass.imported_functions + 1 + pass.functions.len() as u32;
    let mut module = Module::new();
    pass.parse_core_module(&mut module, Parser::new(0), bytes)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let output = module.finish();
    wasmparser::Validator::new_with_features(features).validate_all(&output)?;
    Ok(output)
}

#[derive(Default)]
struct Meter {
    params: Vec<u32>,
    functions: Vec<u32>,
    imported_functions: u32,
    globals: u32,
    helper: u32,
    body: usize,
    imports_added: bool,
    globals_added: bool,
    exports_added: bool,
}

impl Meter {
    fn imports(&mut self, imports: &mut ImportSection) {
        imports.import(
            MODULE,
            ADMIT,
            EntityType::Function(self.params.len() as u32),
        );
        self.imports_added = true;
    }

    fn globals(&mut self, globals: &mut GlobalSection) {
        globals.global(
            GlobalType {
                val_type: ValType::I64,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i64_const(0),
        );
        self.globals_added = true;
    }

    fn exports(&mut self, exports: &mut ExportSection) {
        exports.export(REMAINING_EXPORT, ExportKind::Global, self.globals);
        self.exports_added = true;
    }

    fn charge(&self, function: &mut Function, amount: i64) {
        function.instruction(&Instruction::I64Const(amount));
        function.instruction(&Instruction::Call(self.helper));
    }

    fn flush_segment(
        &mut self,
        function: &mut Function,
        pending: &mut Vec<Operator<'_>>,
    ) -> RewriteResult {
        if pending.is_empty() {
            return Ok(());
        }

        self.charge(function, pending.len() as i64);
        for operator in pending.drain(..) {
            function.instruction(&self.instruction(operator)?);
        }
        Ok(())
    }

    fn charge_bulk(&self, function: &mut Function, temporary: u32, cost: BulkCost) {
        // All supported bulk operations take an unsigned i32 length on top of
        // the operand stack. Preserve it for the original operation.
        function.instruction(&Instruction::LocalTee(temporary));
        function.instruction(&Instruction::LocalGet(temporary));
        function.instruction(&Instruction::I64ExtendI32U);
        match cost {
            BulkCost::Bytes => {
                function.instruction(&Instruction::I64Const(MEMORY_BYTES_PER_GAS - 1));
                function.instruction(&Instruction::I64Add);
                function.instruction(&Instruction::I64Const(MEMORY_BYTES_PER_GAS));
                function.instruction(&Instruction::I64DivU);
            }
            BulkCost::Pages => {
                function.instruction(&Instruction::I64Const(PAGE_GAS));
                function.instruction(&Instruction::I64Mul);
            }
            BulkCost::Elements => {}
        }
        function.instruction(&Instruction::I64Const(1));
        function.instruction(&Instruction::I64Add);
        function.instruction(&Instruction::Call(self.helper));
    }

    fn helper_body(&self) -> Function {
        let mut f = Function::new([]);
        for instruction in [
            Instruction::GlobalGet(self.globals),
            Instruction::LocalGet(0),
            Instruction::I64LtU,
            Instruction::If(BlockType::Empty),
            Instruction::GlobalGet(self.globals),
            Instruction::LocalGet(0),
            Instruction::Call(self.imported_functions),
            Instruction::GlobalSet(self.globals),
            Instruction::End,
            Instruction::GlobalGet(self.globals),
            Instruction::LocalGet(0),
            Instruction::I64Sub,
            Instruction::GlobalSet(self.globals),
            Instruction::End,
        ] {
            f.instruction(&instruction);
        }
        f
    }
}

impl Reencode for Meter {
    type Error = String;

    fn function_index(&mut self, index: u32) -> RewriteResult<u32> {
        Ok(index + u32::from(index >= self.imported_functions))
    }

    fn parse_type_section(
        &mut self,
        types: &mut TypeSection,
        section: wasmparser::TypeSectionReader<'_>,
    ) -> RewriteResult {
        reencode::utils::parse_type_section(self, types, section)?;
        types
            .ty()
            .function([ValType::I64, ValType::I64], [ValType::I64]);
        types.ty().function([ValType::I64], []);
        Ok(())
    }

    fn parse_import_section(
        &mut self,
        imports: &mut ImportSection,
        section: wasmparser::ImportSectionReader<'_>,
    ) -> RewriteResult {
        reencode::utils::parse_import_section(self, imports, section)?;
        self.imports(imports);
        Ok(())
    }

    fn parse_function_section(
        &mut self,
        functions: &mut FunctionSection,
        section: wasmparser::FunctionSectionReader<'_>,
    ) -> RewriteResult {
        reencode::utils::parse_function_section(self, functions, section)?;
        functions.function(self.params.len() as u32 + 1);
        Ok(())
    }

    fn parse_global_section(
        &mut self,
        globals: &mut GlobalSection,
        section: wasmparser::GlobalSectionReader<'_>,
    ) -> RewriteResult {
        reencode::utils::parse_global_section(self, globals, section)?;
        self.globals(globals);
        Ok(())
    }

    fn parse_export_section(
        &mut self,
        exports: &mut ExportSection,
        section: wasmparser::ExportSectionReader<'_>,
    ) -> RewriteResult {
        reencode::utils::parse_export_section(self, exports, section)?;
        self.exports(exports);
        Ok(())
    }

    fn intersperse_section_hook(
        &mut self,
        module: &mut Module,
        _: Option<SectionId>,
        before: Option<SectionId>,
    ) -> RewriteResult {
        if !self.imports_added && !matches!(before, Some(SectionId::Type | SectionId::Import)) {
            let mut imports = ImportSection::new();
            self.imports(&mut imports);
            module.section(&imports);
        }
        if !self.globals_added
            && !matches!(
                before,
                Some(
                    SectionId::Type
                        | SectionId::Import
                        | SectionId::Function
                        | SectionId::Table
                        | SectionId::Memory
                        | SectionId::Tag
                        | SectionId::Global
                )
            )
        {
            let mut globals = GlobalSection::new();
            self.globals(&mut globals);
            module.section(&globals);
        }
        if !self.exports_added
            && !matches!(
                before,
                Some(
                    SectionId::Type
                        | SectionId::Import
                        | SectionId::Function
                        | SectionId::Table
                        | SectionId::Memory
                        | SectionId::Tag
                        | SectionId::Global
                        | SectionId::Export
                )
            )
        {
            let mut exports = ExportSection::new();
            self.exports(&mut exports);
            module.section(&exports);
        }
        Ok(())
    }

    fn parse_code_section(
        &mut self,
        code: &mut CodeSection,
        section: wasmparser::CodeSectionReader<'_>,
    ) -> RewriteResult {
        reencode::utils::parse_code_section(self, code, section)?;
        code.function(&self.helper_body());
        Ok(())
    }

    fn parse_function_body(
        &mut self,
        code: &mut CodeSection,
        body: wasmparser::FunctionBody<'_>,
    ) -> RewriteResult {
        let mut locals = Vec::new();
        let mut temporary = self.params[self.functions[self.body] as usize];
        self.body += 1;
        for local in body.get_locals_reader()? {
            let (count, ty) = local?;
            temporary += count;
            locals.push((count, self.val_type(ty)?));
        }
        locals.push((1, ValType::I32));
        let mut f = Function::new(locals);
        let mut pending = Vec::new();
        let mut operators = body.get_operators_reader()?;
        while !operators.eof() {
            let op = operators.read()?;
            if matches!(
                op,
                Operator::Block { .. }
                    | Operator::Loop { .. }
                    | Operator::If { .. }
                    | Operator::Else
                    | Operator::End
            ) {
                self.flush_segment(&mut f, &mut pending)?;
                f.instruction(&self.instruction(op)?);
                continue;
            }
            let bulk = match op {
                Operator::MemoryCopy { .. }
                | Operator::MemoryFill { .. }
                | Operator::MemoryInit { .. } => Some(BulkCost::Bytes),
                Operator::TableCopy { .. }
                | Operator::TableFill { .. }
                | Operator::TableInit { .. }
                | Operator::TableGrow { .. } => Some(BulkCost::Elements),
                Operator::MemoryGrow { .. } => Some(BulkCost::Pages),
                _ => None,
            };
            if let Some(cost) = bulk {
                self.flush_segment(&mut f, &mut pending)?;
                self.charge_bulk(&mut f, temporary, cost);
                f.instruction(&self.instruction(op)?);
                continue;
            }
            // A call may suspend arbitrarily deep in another function. Charge
            // the continuation after it returns, within that tick's allowance.
            let boundary = matches!(
                op,
                Operator::Br { .. }
                    | Operator::BrIf { .. }
                    | Operator::BrTable { .. }
                    | Operator::Return
                    | Operator::Call { .. }
                    | Operator::CallIndirect { .. }
                    | Operator::ReturnCall { .. }
                    | Operator::ReturnCallIndirect { .. }
                    | Operator::Unreachable
            );
            pending.push(op);
            if boundary || pending.len() >= MAX_SEGMENT_OPERATORS {
                self.flush_segment(&mut f, &mut pending)?;
            }
        }
        self.flush_segment(&mut f, &mut pending)?;
        code.function(&f);
        Ok(())
    }
}

#[cfg(test)]
#[path = "metering_tests.rs"]
mod tests;
