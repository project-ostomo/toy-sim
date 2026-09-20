use super::*;
use toy_sim_model::chat::{MAX_MESSAGE_BYTES, MAX_PAGE_MESSAGES};
use toy_sim_model::llm::{
    LlmRequest, LlmStatus, LlmSubmission, MAX_PROMPT_BYTES, MAX_RESULT_BYTES,
};
use toy_sim_ship_api::services as s;

const SERVICE_GAS: u64 = 8192;

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
        |mut caller: Caller<'_, Host>, id: u64, pointer: u32, bytes: u32, max_tokens: u32| {
            Ok(CallPlan::bytes(&caller, pointer, bytes, MAX_PROMPT_BYTES)?.work(SERVICE_GAS))
        },
        {
            finish((|| {
                let prompt = std::str::from_utf8(payload(&caller, pointer, bytes)?)
                    .map_err(|_| w::ERR_ARGUMENT)?
                    .to_owned();
                let request = LlmRequest {
                    id,
                    prompt,
                    max_tokens,
                };
                if !request.valid() {
                    return Err(w::ERR_ARGUMENT.into());
                }
                let reply = services(caller.data())?.llm_submit(request);
                Ok(match reply {
                    LlmSubmission::Accepted => s::LLM_ACCEPTED,
                    LlmSubmission::AlreadyKnown => s::LLM_ALREADY_KNOWN,
                    LlmSubmission::Unavailable => s::LLM_UNAVAILABLE,
                    LlmSubmission::Busy => s::LLM_BUSY,
                    LlmSubmission::InsufficientGas => s::LLM_INSUFFICIENT_GAS,
                    LlmSubmission::InvalidRequest => s::LLM_INVALID_REQUEST,
                })
            })())
        },
    )?;

    metered!(
        linker,
        "llm_poll",
        |mut caller: Caller<'_, Host>, id: u64, output: u32, capacity: u32, metadata: u32| {
            if id == 0 {
                return Err(w::ERR_ARGUMENT.into());
            }
            memory_range(&caller, output, capacity)?;
            Ok(
                CallPlan::record::<s::LlmPoll>(&caller, metadata, size_of::<s::LlmPoll>() as u32)?
                    .work(SERVICE_GAS + words(MAX_RESULT_BYTES)),
            )
        },
        {
            finish((|| {
                let reply = services(caller.data())?.llm_poll(id);
                let (state, text) = match &reply {
                    LlmStatus::Unknown => (s::LLM_UNKNOWN, ""),
                    LlmStatus::Pending => (s::LLM_PENDING, ""),
                    LlmStatus::Ready { text } => (s::LLM_READY, text.as_str()),
                    LlmStatus::Failed { reason } => (s::LLM_FAILED, reason.as_str()),
                    LlmStatus::Cancelled => (s::LLM_CANCELLED, ""),
                    LlmStatus::Indeterminate => (s::LLM_INDETERMINATE, ""),
                };
                if text.len() > MAX_RESULT_BYTES {
                    return Err(w::ERR_LIMIT.into());
                }
                caller.data_mut().native_credit = words(MAX_RESULT_BYTES) - words(text.len());
                copy_reply(&mut caller, output, capacity, text.as_bytes())?;
                emit(
                    &mut caller,
                    metadata,
                    size_of::<s::LlmPoll>() as u32,
                    &s::LlmPoll {
                        state,
                        bytes: text.len() as u32,
                    },
                )?;
                Ok(0)
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
        |mut caller: Caller<'_, Host>, after: u64, output: u32, capacity: u32, metadata: u32| {
            if capacity == 0 {
                return Err(w::ERR_ARGUMENT.into());
            }
            let count = capacity.min(MAX_PAGE_MESSAGES as u32);
            let bytes = count as usize * size_of::<s::ChatMessage>();
            memory_range(&caller, output, bytes as u32)?;
            Ok(
                CallPlan::record::<s::ChatPage>(
                    &caller,
                    metadata,
                    size_of::<s::ChatPage>() as u32,
                )?
                .work(SERVICE_GAS + words(bytes)),
            )
        },
        {
            finish((|| {
                let limit = capacity.min(MAX_PAGE_MESSAGES as u32);
                let reply = services(caller.data())?.chat_read(after, limit)?;
                if reply.messages.len() > limit as usize {
                    return Err(w::ERR_LIMIT.into());
                }
                let mut records = Vec::with_capacity(reply.messages.len());
                for message in &reply.messages {
                    let mut record = s::ChatMessage {
                        id: message.id.0,
                        sequence: message.sequence,
                        tick: message.tick,
                        calendar_unix_ms: message.calendar_unix_ms,
                        ..Default::default()
                    };
                    if message.sender_name.len() > record.sender.len()
                        || message.text.len() > record.text.len()
                    {
                        return Err(w::ERR_LIMIT.into());
                    }
                    record.sender_bytes = message.sender_name.len() as u32;
                    record.text_bytes = message.text.len() as u32;
                    record.sender[..message.sender_name.len()]
                        .copy_from_slice(message.sender_name.as_bytes());
                    record.text[..message.text.len()].copy_from_slice(message.text.as_bytes());
                    if let Some(owner) = message.advertised_owner {
                        record.flags |= s::CHAT_OWNER_PRESENT;
                        record.owner = owner.0;
                    }
                    if let Some(organization) = message.advertised_organization {
                        record.flags |= s::CHAT_ORGANIZATION_PRESENT;
                        record.organization = organization.0;
                    }
                    records.push(record);
                }
                let stride = size_of::<s::ChatMessage>() as u32;
                for (index, record) in records.iter().enumerate() {
                    emit(&mut caller, output + index as u32 * stride, stride, record)?;
                }
                caller.data_mut().native_credit = words(limit as usize * stride as usize)
                    - words(records.len() * stride as usize);
                emit(
                    &mut caller,
                    metadata,
                    size_of::<s::ChatPage>() as u32,
                    &s::ChatPage {
                        next_sequence: reply.next_sequence,
                        missed: reply.missed,
                        count: records.len() as u32,
                        reserved: 0,
                    },
                )?;
                Ok(0)
            })())
        },
    )?;
    Ok(())
}
