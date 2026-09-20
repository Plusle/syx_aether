struct VSInput {
    [[vk::location(0)]] float3 inPos : POSITION;
    [[vk::location(1)]] float3 inNormal : NORMAL;
    [[vk::location(2)]] float3 inTranslation : TRANSLATION;
    [[vk::location(3)]] float3 inPathNormal : PATH_NORMAL;
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
    float4x4 vp;
    float time;
};

[[vk::binding(0, 0)]] ConstantBuffer<LightingData> lighting : register(b0, space0);
[[vk::binding(0, 1)]] ConstantBuffer<MaterialData> material : register(b0, space1);
[[vk::push_constant]] ConstantBuffer<PushConstants> runtime;

// Rodrigues' rotation formula: rotate point p around axis k by angle theta
float3 rotateAroundAxis(float3 p, float3 k, float theta) {
    float c = cos(theta);
    float s = sin(theta);
    return p * c + cross(k, p) * s + k * dot(k, p) * (1.0 - c);
}

[shader("vertex")]
VSOutput vsMain(VSInput input) {
    VSOutput output;

    // Scale cube to 1/4 size
    float3 localPos = input.inPos * 0.25;

    // Self-rotation around instance's unique axis, uniform speed
    float selfAngle = runtime.time * 0.5;
    float3 selfRotated = rotateAroundAxis(localPos, normalize(input.inPathNormal), selfAngle);
    float3 selfRotNormal = rotateAroundAxis(input.inNormal, normalize(input.inPathNormal), selfAngle);

    // Orbit around origin along the circle defined by inPathNormal.
    float orbitAngle = runtime.time * 0.2;
    float3 anchored = rotateAroundAxis(input.inTranslation, normalize(input.inPathNormal), orbitAngle);

    float4 world = float4(anchored + selfRotated, 1.0);
    output.outWorldPos = float4(world.xyz, 1.0);
    output.outNormal = float4(normalize(selfRotNormal), 1.0);

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
