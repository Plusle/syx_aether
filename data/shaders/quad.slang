struct VSInput {
    [[vk::location(0)]] float3 inPos : POSITION;
    [[vk::location(1)]] float2 inUV : TEXCOORD0;
};

struct VSOutput {
    float4 outPos : SV_POSITION;
    [[vk::location(0)]] float2 outUV : TEXCOORD0;
};

struct PushConstants {
    float4x4 mvp;
};

[[vk::push_constant]] ConstantBuffer<PushConstants> g_push;

struct MaterialData {
    int texIndex;
    int samplerIndex;
};

[[vk::binding(0, 1)]] ConstantBuffer<MaterialData> material : register(b0, space1);

[[vk::binding(0, 2)]] Texture2D<float4> g_textures[];
[[vk::binding(1, 2)]] SamplerState g_samplers[];

[shader("vertex")]
VSOutput vsMain(VSInput input) {
    VSOutput output;
    output.outPos = mul(g_push.mvp, float4(input.inPos, 1.0));
    output.outUV = input.inUV;
    return output;
}

[shader("fragment")]
float4 fsMain(VSOutput input) : SV_TARGET {
    return g_textures[material.texIndex].Sample(g_samplers[material.samplerIndex], input.outUV);
}
