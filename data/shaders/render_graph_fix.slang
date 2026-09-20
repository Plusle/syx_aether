struct PushConstants {
    uint mode; // 0: pass A (slow writer of 1s), 1: pass B (read + increment)
    uint spin; // dummy loop iterations for pass A to widen the race window
};

[[vk::binding(0, 0)]] RWStructuredBuffer<uint> counters : register(u0, space0);
[[vk::push_constant]] ConstantBuffer<PushConstants> g_push;

[shader("compute")]
[numthreads(64, 1, 1)]
void csMain(uint tid : SV_DispatchThreadID) {
    if (g_push.mode == 0) {
        // Burn GPU time so that, without an inter-pass barrier, pass B's
        // dispatch overlaps this one and reads stale values. The compiler
        // cannot prove this loop dead.
        uint acc = tid * 2654435761u;
        for (uint i = 0; i < g_push.spin; ++i) {
            acc = acc * 1664525u + 1013904223u;
        }
        if (acc == 0xDEADBEEFu) {
            counters[tid] = acc;
        }
        counters[tid] = 1;
    } else {
        counters[tid] = counters[tid] + 1;
    }
}
