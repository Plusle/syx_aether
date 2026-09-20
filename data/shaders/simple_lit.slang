struct VSInput {
    [[vk::location(0)]] float3 inPos : POSITION;
    [[vk::location(1)]] float3 inNormal : NORMAL;
};

struct VSOutput {
    float4 outWorldPos : POSITION0;
    float4 outNormal : NORMAL0;
    float4 outPosition : SV_POSITION;
};

struct LightingData {
    float3 lightDir;    float _pad0;
    float3 lightColor;  float lightIntensity;
    float _pad1; float _pad2;
};

struct MaterialData {
    float ambient; float _pad0; float _pad1; float _pad2;
};

struct PushConstants {
    float4x4 model;
    float4x4 vp;
};

[[vk::binding(0, 0)]] ConstantBuffer<LightingData> lighting : register(b0, space0);
[[vk::binding(0, 1)]] ConstantBuffer<MaterialData> material : register(b0, space1);
[[vk::push_constant]] ConstantBuffer<PushConstants> runtime;

[shader("vertex")]
VSOutput vsMain(VSInput input) {
    VSOutput output;
    float4 world = mul(runtime.model, float4(input.inPos, 1.0));
    output.outWorldPos = float4(world.xyz, 1.0);
    output.outNormal = float4(normalize(mul((float3x3)runtime.model, input.inNormal)), 1.0);
    output.outPosition = mul(runtime.vp, world);
    return output;
}

[shader("fragment")]
float4 fsMain(VSOutput input) : SV_TARGET {
    float NdotL = max(dot(normalize(input.outNormal.xyz), normalize(-lighting.lightDir)), 0.0);
    float3 ambient = float3(material.ambient * 0.5);
    float3 diffuse = NdotL * lighting.lightColor * lighting.lightIntensity;
    return float4(ambient + diffuse, 1.0);
}
