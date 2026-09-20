struct VSInput {
    [[vk::location(0)]] float3 inWorldPos : POSITION;
    [[vk::location(1)]] float3 inNormal : NORMAL;
};

struct VSOutput {
    float4 outPosition : SV_POSITION;
};

struct PushConstants {
    float4x4 model;
    float4x4 lightVP;
};

[[vk::push_constant]] ConstantBuffer<PushConstants> runtime;

[shader("vertex")]
VSOutput vsMain(VSInput input) {
    VSOutput output;
    float4 world = mul(runtime.model, float4(input.inWorldPos, 1.0));
    output.outPosition = mul(runtime.lightVP, world);
    return output;
}

[shader("fragment")]
void fsMain(VSOutput input) {
    // Depth-only
}
