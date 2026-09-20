struct VSInput {
    [[vk::location(0)]] float3 inPos : POSITION;
};

struct VSOutput {
    float4 outPos : SV_POSITION;
    [[vk::location(0)]] float3 outColor : COLOR;
};

[shader("vertex")]
VSOutput vsMain(VSInput input, uint vertexIndex : SV_VertexID) {
    VSOutput output;
    output.outPos = float4(input.inPos, 1.0);
    float3 colors[3] = {
        float3(1.0, 0.0, 0.0),
        float3(0.0, 1.0, 0.0),
        float3(0.0, 0.0, 1.0),
    };
    output.outColor = colors[vertexIndex];
    return output;
}

[shader("fragment")]
float4 fsMain(VSOutput input) : SV_TARGET {
    return float4(input.outColor, 1.0);
}
