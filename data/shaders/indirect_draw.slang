struct VSInput {
    [[vk::location(0)]] float4 pos;
    [[vk::location(1)]] float4 normal;
}

struct VSOutput {
    float4 outPos : SV_Position;
    float4 outWorld : POSITION0;
    float4 outNormal : NORMAL0;
}

struct InstanceData {
    float4 axis;
    float4 translation;
}

struct PushConstants {
    float4x4 vp;
    float time;
}

struct LightingData {
    float3 lightDir;    float _pad0;
    float3 lightColor;  float lightIntensity;
    float _pad1; float _pad2;
}

struct MaterialData {
    float ambient; float _pad0; float _pad1; float _pad2;
}

[[vk::binding(0, 0)]] ConstantBuffer<LightingData> lighting : register(b0, space0);
[[vk::binding(1, 0)]] StructuredBuffer<InstanceData> instances : register(t0, space0);
[[vk::binding(2, 0)]] StructuredBuffer<uint> indices : register(t1, space0);
[[vk::binding(0, 1)]] ConstantBuffer<MaterialData> material : register(b1, space0);
[[vk::push_constant]] ConstantBuffer<PushConstants> runtime;

float3 rotateAroundAxis(float3 p, float3 k, float theta) {
    float c = cos(theta);
    float s = sin(theta);
    return p * c + cross(k, p) * s + k * dot(k, p) * (1.0 - c);
}

[shader("vertex")]
VSOutput vsMain(VSInput input, uint threadID : SV_InstanceID) {
    VSOutput output;
    uint index = indices[threadID];
    InstanceData instance = instances[index];

    // Scale cube to 1/4 size
    float3 localPos = input.pos.xyz * 0.25;

    // Self-rotation around instance's unique axis, uniform speed
    float selfAngle = runtime.time * 0.5;
    float3 selfRotated = rotateAroundAxis(localPos, normalize(instance.axis.xyz), selfAngle);
    float3 selfRotNormal = rotateAroundAxis(input.normal.xyz, normalize(instance.axis.xyz), selfAngle);

    // Orbit around origin along the circle defined by inPathNormal.
    float orbitAngle = runtime.time * 0.2;
    float3 anchored = rotateAroundAxis(instance.translation.xyz, normalize(instance.axis.xyz), orbitAngle);

    float4 world = float4(anchored + selfRotated, 1.0);

    output.outWorld = float4(world.xyz, 1.0);
    output.outNormal = float4(normalize(selfRotNormal), 1.0);
    output.outPos = mul(runtime.vp, world);

    return output;
}

[shader("fragment")]
float4 fsMain(VSOutput input) : SV_Target {
    float NdotL = max(dot(normalize(input.outNormal.xyz), normalize(-lighting.lightDir)), 0.0);
    float3 ambient = float3(material.ambient * 0.5);
    float3 diffuse = NdotL * lighting.lightColor * lighting.lightIntensity;
    return float4(ambient + diffuse, 1.0);
}
