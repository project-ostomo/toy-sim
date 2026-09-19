use super::*;
use toy_sim_model::chat::{MAX_MESSAGE_BYTES, MAX_PAGE_MESSAGES};
use toy_sim_model::llm::{LlmRequest, MAX_PROMPT_BYTES, MAX_RESULT_BYTES};

const LLM_REQUEST_BYTES: usize = MAX_PROMPT_BYTES + 32;
const LLM_REPLY_BYTES: usize = MAX_RESULT_BYTES + 32;
const SERVICE_GAS: u64 = 8192;
const CHAT_REPLY_BYTES: usize = 65536;

fn services(host: &Host) -> CallResult<Arc<dyn ProgramServices>> {
    host.services
        .clone()
        .ok_or_else(|| w::ERR_UNAVAILABLE.into())
}

fn copy_reply(
    caller: &mut Caller<'_, Host>,
    pointer: u32,
    capacity: u32,
    bytes: &[u8],
) -> CallResult<i32> {
    if bytes.len() > capacity as usize {
        return Err(w::ERR_BUFFER.into());
    }
    emit_bytes(caller, pointer, bytes.len() as u32, bytes)?;
    Ok(bytes.len() as i32)
}

pub(super) fn register(linker: &mut Linker<Host>) -> Result<()> {
    metered!(
        linker,
        "llm_submit",
        |mut caller: Caller<'_, Host>, pointer: u32, bytes: u32, output: u32, capacity: u32| {
            if capacity < 1 {
                return Err(w::ERR_BUFFER.into());
            }
            memory_range(&caller, output, 1)?;
            Ok(CallPlan::bytes(&caller, pointer, bytes, LLM_REQUEST_BYTES)?.work(SERVICE_GAS))
        },
        {
            finish((|| {
                let request: LlmRequest = postcard::from_bytes(payload(&caller, pointer, bytes)?)
                    .map_err(|_| w::ERR_ARGUMENT)?;
                if !request.valid() {
                    return Err(w::ERR_ARGUMENT.into());
                }
                let reply = services(caller.data())?.llm_submit(request);
                let bytes = postcard::to_stdvec(&reply).map_err(|_| w::ERR_LIMIT)?;
                copy_reply(&mut caller, output, capacity, &bytes)
            })())
        },
    )?;

    metered!(
        linker,
        "llm_poll",
        |mut caller: Caller<'_, Host>, id: u64, output: u32, capacity: u32| {
            if id == 0 {
                return Err(w::ERR_ARGUMENT.into());
            }
            Ok(CallPlan::bytes(&caller, output, capacity, LLM_REPLY_BYTES)?
                .work(SERVICE_GAS + words(LLM_REPLY_BYTES) - words(capacity as usize)))
        },
        {
            finish((|| {
                let reply = services(caller.data())?.llm_poll(id);
                let bytes = postcard::to_stdvec(&reply).map_err(|_| w::ERR_LIMIT)?;
                if bytes.len() > LLM_REPLY_BYTES {
                    return Err(w::ERR_LIMIT.into());
                }
                caller.data_mut().native_credit = words(LLM_REPLY_BYTES) - words(bytes.len());
                copy_reply(&mut caller, output, capacity, &bytes)
            })())
        },
    )?;

    metered!(
        linker,
        "llm_cancel",
        |mut caller: Caller<'_, Host>, id: u64| {
            if id == 0 {
                return Err(w::ERR_ARGUMENT.into());
            }
            Ok(CallPlan::fixed()?.work(SERVICE_GAS))
        },
        { finish(services(caller.data()).map(|service| i32::from(service.llm_cancel(id)))) },
    )?;

    metered!(
        linker,
        "chat_send",
        |mut caller: Caller<'_, Host>, id: u64, pointer: u32, bytes: u32| {
            if id == 0 {
                return Err(w::ERR_ARGUMENT.into());
            }
            Ok(CallPlan::bytes(&caller, pointer, bytes, MAX_MESSAGE_BYTES)?
                .work(services(caller.data())?.chat_send_work()))
        },
        {
            status((|| {
                let text = std::str::from_utf8(payload(&caller, pointer, bytes)?)
                    .map_err(|_| w::ERR_ARGUMENT)?;
                if text.trim().is_empty() {
                    return Err(w::ERR_ARGUMENT.into());
                }
                services(caller.data())?
                    .chat_send(id, text)
                    .map_err(Into::into)
            })())
        },
    )?;

    metered!(
        linker,
        "chat_read",
        |mut caller: Caller<'_, Host>, after: u64, limit: u32, output: u32, capacity: u32| {
            if limit == 0 || limit as usize > MAX_PAGE_MESSAGES {
                return Err(w::ERR_ARGUMENT.into());
            }
            Ok(
                CallPlan::bytes(&caller, output, capacity, CHAT_REPLY_BYTES)?
                    .work(SERVICE_GAS + words(CHAT_REPLY_BYTES) - words(capacity as usize)),
            )
        },
        {
            finish((|| {
                let reply = services(caller.data())?.chat_read(after, limit)?;
                let bytes = postcard::to_stdvec(&reply).map_err(|_| w::ERR_LIMIT)?;
                if bytes.len() > CHAT_REPLY_BYTES {
                    return Err(w::ERR_LIMIT.into());
                }
                caller.data_mut().native_credit = words(CHAT_REPLY_BYTES) - words(bytes.len());
                copy_reply(&mut caller, output, capacity, &bytes)
            })())
        },
    )?;
    Ok(())
}
