struct VSOutput {
    [[vk::location(0)]] float2 outUV : TEXCOORD0;
    float4 outPosition : SV_POSITION;
};

static const float2 positions[6] = {
    float2(-1.0, -1.0), float2( 1.0, -1.0), float2(-1.0,  1.0),
    float2( 1.0, -1.0), float2( 1.0,  1.0), float2(-1.0,  1.0),
};

static const float2 uvs[6] = {
    float2(0.0, 0.0), float2(1.0, 0.0), float2(0.0, 1.0),
    float2(1.0, 0.0), float2(1.0, 1.0), float2(0.0, 1.0),
};

struct LightingData {
    float3 lightDir;    float _pad0;
    float3 lightColor;  float lightIntensity;
    float _pad1; float _pad2;
};

struct MaterialData {
    int positionIndex;
    int normalIndex;
    int samplerIndex;
    int _pad;
};

[[vk::binding(0, 0)]] ConstantBuffer<LightingData> lighting : register(b0, space0);
[[vk::binding(0, 1)]] ConstantBuffer<MaterialData> material : register(b0, space1);

[[vk::binding(0, 2)]] Texture2D<float4> g_textures[];
[[vk::binding(1, 2)]] SamplerState g_samplers[];

[shader("vertex")]
VSOutput vsMain(uint vertexIndex : SV_VertexID) {
    VSOutput output;
    output.outUV = uvs[vertexIndex];
    output.outPosition = float4(positions[vertexIndex], 0.0, 1.0);
    return output;
}

[shader("fragment")]
float4 fsMain(VSOutput input) : SV_TARGET {
    float4 position = g_textures[material.positionIndex].Sample(g_samplers[material.samplerIndex], input.outUV);
    float4 normal = g_textures[material.normalIndex].Sample(g_samplers[material.samplerIndex], input.outUV);

    float NdotL = max(dot(normalize(normal.xyz), normalize(-lighting.lightDir)), 0.0);

    float3 ambient = float3(0.4);
    float3 diffuse = NdotL * lighting.lightColor * lighting.lightIntensity;
    if (all(position.xyz == float3(0, 0, 0)))
        discard;
    return float4(ambient + diffuse, 1.0);
}
