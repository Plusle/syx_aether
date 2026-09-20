struct VSInput {
    [[vk::location(0)]] float3 inPos : POSITION;
    [[vk::location(1)]] float3 inNormal : NORMAL;
};

struct VSOutput {
    float4 outWorldPos : POSITION0;
    float4 outNormal : NORMAL0;
    float4 outLightClipPos : POSITION1;
    float4 outPosition : SV_POSITION;
};

struct LightingData {
    float3 lightDir;    float _pad0;
    float3 lightColor;  float lightIntensity;
    float3 lightPos;    float _pad1;
};

struct MaterialData {
    float ambient; float _pad0; float _pad1; float _pad2;
};

struct ShadowMaterialData {
    int shadowMapIndex;
    int shadowSamplerIndex;
    float4x4 lightVP;
};

struct PushConstants {
    float4x4 model;
    float4x4 vp;
};

[[vk::binding(0, 0)]] ConstantBuffer<LightingData> lighting : register(b0, space0);
[[vk::binding(0, 1)]] ConstantBuffer<MaterialData> material : register(b0, space1);
[[vk::binding(1, 1)]] ConstantBuffer<ShadowMaterialData> shadowMat : register(b1, space1);
[[vk::push_constant]] ConstantBuffer<PushConstants> runtime;

[[vk::binding(0, 2)]] Texture2D<float4> g_textures[];
[[vk::binding(1, 2)]] SamplerState g_samplers[];

float sampleShadow(float4 lightClipPos) {
    float3 projCoords = lightClipPos.xyz / lightClipPos.w;
    projCoords.xy = projCoords.xy * 0.5 + 0.5;

    if (projCoords.x < 0.0 || projCoords.x > 1.0 || projCoords.y < 0.0 || projCoords.y > 1.0)
        return 1.0;

    // Normalize depth from light clip space to [0, 1] range.
    // The light uses an orthographic projection with near=0.1, far=30.0.
    float linearDepth = projCoords.z;
    float shadowDepth = g_textures[shadowMat.shadowMapIndex].Sample(
        g_samplers[shadowMat.shadowSamplerIndex], projCoords.xy
    ).r;

    return linearDepth <= shadowDepth + 0.005 ? 1.0 : 0.0;
}

[shader("vertex")]
VSOutput vsMain(VSInput input) {
    VSOutput output;
    float4 world = mul(runtime.model, float4(input.inPos, 1.0));
    output.outWorldPos = float4(world.xyz, 1.0);
    output.outNormal = float4(normalize(mul((float3x3)runtime.model, input.inNormal)), 1.0);
    output.outPosition = mul(runtime.vp, world);
    output.outLightClipPos = mul(shadowMat.lightVP, world);
    return output;
}

[shader("fragment")]
float4 fsMain(VSOutput input) : SV_TARGET {
    float NdotL = max(dot(normalize(input.outNormal.xyz), normalize(-lighting.lightDir)), 0.0);
    float shadow = sampleShadow(input.outLightClipPos);
    float3 ambient = float3(material.ambient * 0.5);
    float3 diffuse = NdotL * lighting.lightColor * lighting.lightIntensity * shadow;
    return float4(ambient + diffuse, 1.0);
}
