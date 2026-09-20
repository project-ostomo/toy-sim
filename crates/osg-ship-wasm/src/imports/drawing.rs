use super::*;

fn color(value: u64) -> CallResult<Ink> {
    if value > 0xff_ffff {
        return Err(w::ERR_ARGUMENT.into());
    }

    Ok([(value >> 16) as u8, (value >> 8) as u8, value as u8])
}

fn point(x: i64, y: i64) -> CallResult<[i16; 2]> {
    Ok([
        x.try_into().map_err(|_| w::ERR_ARGUMENT)?,
        y.try_into().map_err(|_| w::ERR_ARGUMENT)?,
    ])
}

fn extent(x: u64, y: u64) -> CallResult<[u16; 2]> {
    if x > u64::from(u16::MAX) || y > u64::from(u16::MAX) {
        return Err(w::ERR_ARGUMENT.into());
    }

    Ok([x as u16, y as u16])
}

fn draw(kind: u64, parameters: &[u8], payload: &[u8]) -> CallResult<Draw> {
    if !matches!(kind, w::DRAW_TEXT | w::DRAW_POLYLINE) && !payload.is_empty() {
        return Err(w::ERR_BUFFER.into());
    }

    Ok(match kind {
        w::DRAW_PIXEL => {
            let value = w::ScreenPixel::read(parameters).ok_or(w::ERR_BUFFER)?;
            Draw::Pixel {
                at: point(value.x, value.y)?,
                color: color(value.color)?,
            }
        }
        w::DRAW_TEXT => {
            let value = w::ScreenText::read(parameters).ok_or(w::ERR_BUFFER)?;

            if payload.len() > 4096 {
                return Err(w::ERR_LIMIT.into());
            }

            Draw::Text {
                at: point(value.x, value.y)?,
                color: color(value.color)?,
                text: std::str::from_utf8(payload)
                    .map_err(|_| w::ERR_ARGUMENT)?
                    .to_owned(),
            }
        }
        w::DRAW_LINE => {
            let value = w::ScreenLine::read(parameters).ok_or(w::ERR_BUFFER)?;
            Draw::Line {
                from: point(value.x1, value.y1)?,
                to: point(value.x2, value.y2)?,
                color: color(value.color)?,
            }
        }
        w::DRAW_POLYLINE => {
            let value = w::ScreenPolyline::read(parameters).ok_or(w::ERR_BUFFER)?;
            let stride = size_of::<w::ScreenPoint>();

            if payload.len() % stride != 0 || !(2..=256).contains(&(payload.len() / stride)) {
                return Err(w::ERR_ARGUMENT.into());
            }

            let points = payload
                .chunks_exact(stride)
                .map(|bytes| {
                    let value = w::ScreenPoint::read(bytes).unwrap();
                    point(value.x, value.y)
                })
                .collect::<CallResult<Vec<_>>>()?;

            Draw::Polyline {
                points,
                color: color(value.color)?,
            }
        }
        w::DRAW_RECTANGLE => {
            let value = w::ScreenRectangle::read(parameters).ok_or(w::ERR_BUFFER)?;

            if value.filled > 1 {
                return Err(w::ERR_ARGUMENT.into());
            }

            Draw::Rect {
                at: point(value.x, value.y)?,
                size: extent(value.width, value.height)?,
                filled: value.filled != 0,
                color: color(value.color)?,
            }
        }
        w::DRAW_ELLIPSE => {
            let value = w::ScreenEllipse::read(parameters).ok_or(w::ERR_BUFFER)?;

            if value.filled > 1 {
                return Err(w::ERR_ARGUMENT.into());
            }

            Draw::Ellipse {
                centre: point(value.cx, value.cy)?,
                radii: extent(value.rx, value.ry)?,
                filled: value.filled != 0,
                color: color(value.color)?,
            }
        }
        _ => return Err(w::ERR_ARGUMENT.into()),
    })
}

pub(super) fn register(linker: &mut Linker<Host>) -> Result<()> {
    metered!(
        linker,
        "screen_define",
        |mut caller: Caller<'_, Host>, pointer: u32, bytes: u32| {
            CallPlan::record::<w::ScreenDefinition>(&caller, pointer, bytes)
        },
        {
            status((|| {
                let value: w::ScreenDefinition = input(&mut caller, pointer, bytes)?;

                if value.id >= u64::from(w::MAX_SCREENS)
                    || !(32..=2048).contains(&value.width)
                    || !(32..=2048).contains(&value.height)
                    || value.title.as_str().is_none_or(str::is_empty)
                {
                    return Err(w::ERR_ARGUMENT.into());
                }

                let host = caller.data_mut();
                let resized = host.working.screens.iter().any(|screen| {
                    screen.id == value.id
                        && (screen.width != value.width || screen.height != value.height)
                });
                host.working.screens.retain(|screen| screen.id != value.id);
                host.working.screens.push(value);

                if resized {
                    host.drafts.remove(&value.id);
                    host.output
                        .screens
                        .retain(|frame| u64::from(frame.screen_id) != value.id);
                    clear_screen(host, value.id);
                }

                Ok(())
            })())
        },
    )?;

    metered!(
        linker,
        "screen_remove",
        |mut caller: Caller<'_, Host>, screen: u64| { CallPlan::fixed() },
        {
            status((|| {
                if screen >= u64::from(w::MAX_SCREENS) {
                    return Err(w::ERR_ARGUMENT.into());
                }

                let host = caller.data_mut();
                host.working.screens.retain(|value| value.id != screen);
                host.drafts.remove(&screen);
                host.output
                    .screens
                    .retain(|frame| u64::from(frame.screen_id) != screen);
                clear_screen(host, screen);

                for index in 0..host.events.len() {
                    let event = host.events[index];
                    if event.screen == screen {
                        acknowledge(host, event.id);
                    }
                }

                Ok(())
            })())
        },
    )?;

    metered!(
        linker,
        "screen_begin",
        |mut caller: Caller<'_, Host>, pointer: u32, bytes: u32| {
            CallPlan::record::<w::ScreenFrame>(&caller, pointer, bytes)
        },
        {
            status((|| {
                let value: w::ScreenFrame = input(&mut caller, pointer, bytes)?;
                let background = color(value.background)?;
                let host = caller.data_mut();
                let definition = host
                    .working
                    .screens
                    .iter()
                    .find(|screen| screen.id == value.id)
                    .ok_or(w::ERR_UNAVAILABLE)?;

                if !host
                    .input
                    .as_ref()
                    .unwrap()
                    .requested_screens
                    .contains(&(value.id as u8))
                {
                    return Err(w::ERR_UNAVAILABLE.into());
                }

                if host.drafts.contains_key(&value.id)
                    || host
                        .output
                        .screens
                        .iter()
                        .any(|frame| u64::from(frame.screen_id) == value.id)
                {
                    return Err(w::ERR_ARGUMENT.into());
                }

                host.drafts.insert(
                    value.id,
                    ScreenDraft {
                        frame: ScreenImage {
                            screen_id: value.id as u8,
                            background,
                            width: definition.width as u16,
                            height: definition.height as u16,
                            draws: Vec::new(),
                            buttons: Default::default(),
                        },
                        payload_bytes: 0,
                        draw_work: 0,
                    },
                );
                Ok(())
            })())
        },
    )?;

    metered!(
        linker,
        "screen_draw",
        |mut caller: Caller<'_, Host>,
         screen: u64,
         kind: u64,
         pointer: u32,
         bytes: u32,
         data_pointer: u32,
         data_bytes: u32| {
            CallPlan::bytes(&caller, pointer, bytes, 48)
                .and_then(|plan| plan.extra_bytes(&caller, data_pointer, data_bytes, 4096))
        },
        {
            status((|| {
                if bytes > 48 || data_bytes > 4096 {
                    return Err(w::ERR_LIMIT.into());
                }

                memory_range(&caller, pointer, bytes)?;
                memory_range(&caller, data_pointer, data_bytes)?;

                let value = draw(
                    kind,
                    payload(&caller, pointer, bytes)?,
                    payload(&caller, data_pointer, data_bytes)?,
                )?;
                let draft = caller
                    .data_mut()
                    .drafts
                    .get_mut(&screen)
                    .ok_or(w::ERR_UNAVAILABLE)?;

                if draft.frame.draws.len() >= w::MAX_SCREEN_PRIMITIVES as usize
                    || draft.payload_bytes + data_bytes as usize > w::MAX_SCREEN_PAYLOAD as usize
                    || draft.draw_work + value.work() > 1_048_576
                {
                    return Err(w::ERR_LIMIT.into());
                }

                draft.draw_work += value.work();
                draft.frame.draws.push(value);
                draft.payload_bytes += data_bytes as usize;
                Ok(())
            })())
        },
    )?;

    metered!(
        linker,
        "screen_button",
        |mut caller: Caller<'_, Host>, screen: u64, key: u64, pointer: u32, bytes: u32| {
            CallPlan::bytes(&caller, pointer, bytes, 24)
        },
        {
            status((|| {
                if key >= 12 || bytes > 24 {
                    return Err(w::ERR_ARGUMENT.into());
                }

                let label = std::str::from_utf8(payload(&caller, pointer, bytes)?)
                    .map_err(|_| w::ERR_ARGUMENT)?
                    .to_owned();

                if label.chars().count() > 6 {
                    return Err(w::ERR_ARGUMENT.into());
                }

                let draft = caller
                    .data_mut()
                    .drafts
                    .get_mut(&screen)
                    .ok_or(w::ERR_UNAVAILABLE)?;
                draft.frame.buttons[key as usize] = (!label.is_empty()).then_some(label);
                Ok(())
            })())
        },
    )?;

    metered!(
        linker,
        "screen_end",
        |mut caller: Caller<'_, Host>, screen: u64| { CallPlan::fixed() },
        {
            status((|| {
                let host = caller.data_mut();
                let draft = host.drafts.get(&screen).ok_or(w::ERR_UNAVAILABLE)?;

                if !draft.frame.valid() {
                    return Err(w::ERR_ARGUMENT.into());
                }

                let draft = host.drafts.remove(&screen).unwrap();
                host.output.screens.push(draft.frame);
                Ok(())
            })())
        },
    )?;

    Ok(())
}
