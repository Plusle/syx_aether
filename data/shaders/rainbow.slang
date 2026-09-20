struct PushConstants {
    float time;
    int imageIndex;
};

[[vk::push_constant]] PushConstants g_push;

[[vk::binding(2, 2)]] RWTexture2D<float4> g_storage_images[];

[numthreads(8, 8, 1)]
[shader("compute")]
void csMain(uint3 dispatchThreadID : SV_DispatchThreadID) {
    uint2 tid = dispatchThreadID.xy;
    uint2 dims;
    g_storage_images[g_push.imageIndex].GetDimensions(dims.x, dims.y);

    float2 uv = (float2)tid / (float2)dims;
    float t = g_push.time;

    float r = 0.5 + 0.5 * sin(uv.x * 6.2831 + t);
    float g = 0.5 + 0.5 * sin(uv.y * 6.2831 + t * 1.3);
    float b = 0.5 + 0.5 * sin((uv.x + uv.y) * 3.1415 + t * 0.7);

    g_storage_images[g_push.imageIndex][tid] = float4(r, g, b, 1.0);
}
