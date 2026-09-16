/* Freestanding C fixture: no libc, allocator, WASI, or serialization. */
#include "ship.h"

static uint64_t callbacks;

void *memset(void *destination, int value, size_t count) {
    volatile unsigned char *bytes = destination;

    for (size_t i = 0; i < count; ++i) {
        bytes[i] = (unsigned char)value;
    }

    return destination;
}

__attribute__((export_name("ship_api_version"))) uint32_t version(void) {
    return SHIP_API_VERSION;
}

__attribute__((export_name("ship_tick"))) void tick(void) {
    ship_tick_context_record context;
    ship_flight_state_record flight;

    if (ship_tick_read(&context, sizeof(context)) < 0) return;
    if (ship_flight_read(&flight, sizeof(flight)) < 0) return;

    ship_throttle_setting_record setting = {
        .fraction = (++callbacks == 1) ? 0.25 : 0.5,
    };
    ship_device_write(1, SHIP_SET_THROTTLE, &setting, sizeof(setting));

    ship_attitude_state_record attitude = {
        .valid_until_s = context.time_s + 1.0,
        .mode = SHIP_ATTITUDE_MANUAL,
    };
    ship_instrument_attitude_put(&attitude, sizeof(attitude));

    ship_spatial_path_record forecast = {
        .meta = {
            .id = 1,
            .role = SHIP_PATH_OWN_FORECAST,
            .valid_until_s = context.time_s + 1.0,
        },
        .frame = {
            .kind = SHIP_FRAME_SNAPSHOT,
            .reference = context.snapshot,
            .origin_velocity_m_s = {
                flight.velocity[0], flight.velocity[1], flight.velocity[2],
            },
        },
        .kind = SHIP_PATH_TIMED,
    };
    ship_spatial_vertex_record vertices[2] = {
        { .time_s = context.time_s },
        { .time_s = context.time_s + 60.0, .position_m = { 0.0, 42.0, 0.0 } },
    };
    ship_spatial_path_put(&forecast, sizeof(forecast), vertices, 2);
}
