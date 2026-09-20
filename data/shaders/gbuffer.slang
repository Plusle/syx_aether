struct VSInput {
    [[vk::location(0)]] float3 inWorldPos : POSITION;
    [[vk::location(1)]] float3 inNormal : NORMAL;
};

struct VSOutput {
    float4 outPosition : POSITION0;
    float4 outNormal : NORMAL0;
    float4 outPositionSV : SV_POSITION;
};

struct PushConstants {
    float4x4 model;
    float4x4 vp;
};

[[vk::push_constant]] ConstantBuffer<PushConstants> runtime;

[shader("vertex")]
VSOutput vsMain(VSInput input) {
    VSOutput output;
    float4 world = mul(runtime.model, float4(input.inWorldPos, 1.0));
    output.outPosition = float4(world.xyz, 1.0);
    output.outNormal = float4(normalize(mul((float3x3)runtime.model, input.inNormal)), 1.0);
    output.outPositionSV = mul(runtime.vp, world);
    return output;
}

struct FSOutput {
    [[vk::location(0)]] float4 position;
    [[vk::location(1)]] float4 normal;
};

[shader("fragment")]
FSOutput fsMain(VSOutput input) {
    FSOutput output;
    output.position = input.outPosition;
    output.normal = input.outNormal;
    return output;
}
