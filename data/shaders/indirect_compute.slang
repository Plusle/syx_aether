struct VertexData {
    float4 pos;
    float2 uv;
}

struct InstanceData {
    float4 axis;
    float4 translation;
};

struct IndirectCommand {
    uint indexCount;
    uint instanceCount;
    uint firstIndex;
    int  offset;
    uint firstInstance;
}

struct PushConstants {
    float4x4 vp;
    uint instances;
    float time;
};

[[vk::binding(0, 0)]] StructuredBuffer<InstanceData> instance : register(t0, space0);
[[vk::binding(1, 0)]] RWStructuredBuffer<uint> count : register(u1, space0);
[[vk::binding(2, 0)]] RWStructuredBuffer<uint> indices : register(u2, space0);
[[vk::binding(3, 0)]] RWStructuredBuffer<IndirectCommand> indirect : register(u3, space0);
[[vk::push_constant]] ConstantBuffer<PushConstants> runtime;

float3 rotateAroundAxis(float3 p, float3 k, float theta) {
    float c = cos(theta);
    float s = sin(theta);
    return p * c + cross(k, p) * s + k * dot(k, p) * (1.0 - c);
}

float3 ComputeInstanceWorldPosition(uint index, float3 localCorner) {
    InstanceData inst = instance[index];
    float3 axis = normalize(inst.axis.xyz);

    // Scale cube to 1/4 size
    float3 localPos = localCorner * 0.25;

    // Self-rotation around instance's unique axis
    float selfAngle = runtime.time * 0.5;
    float3 selfRotated = rotateAroundAxis(localPos, axis, selfAngle);

    // Orbit around origin along the circle defined by axis
    float orbitAngle = runtime.time * 0.2;
    float3 anchored = rotateAroundAxis(inst.translation.xyz, axis, orbitAngle);

    return anchored + selfRotated;
}

bool Culling(uint index) {
    float3 axis = normalize(instance[index].axis.xyz);

    // 8 corners of a unit cube centered at origin
    static const float3 corners[8] = {
        float3(-0.5, -0.5, -0.5), float3( 0.5, -0.5, -0.5),
        float3(-0.5,  0.5, -0.5), float3( 0.5,  0.5, -0.5),
        float3(-0.5, -0.5,  0.5), float3( 0.5, -0.5,  0.5),
        float3(-0.5,  0.5,  0.5), float3( 0.5,  0.5,  0.5),
    };

    float4 clips[8];
    for (uint i = 0; i < 8; i++) {
        float3 worldPos = ComputeInstanceWorldPosition(index, corners[i]);
        clips[i] = mul(runtime.vp, float4(worldPos, 1.0));
    }

    bool allLeft  = true, allRight  = true;
    bool allBottom = true, allTop   = true;
    bool allNear  = true, allFar    = true;

    for (uint i = 0; i < 8; i++) {
        float4 c = clips[i];
        float w = c.w;

        if (c.x >= -w) allLeft   = false;
        if (c.x <=  w) allRight  = false;

        if (c.y >= -w) allBottom = false;
        if (c.y <=  w) allTop    = false;

        // Vulkan/D3D near plane
        if (c.z >= 0.0) allNear  = false;

        // Far plane
        if (c.z <= w) allFar     = false;
    }

    return allLeft || allRight || allBottom || allTop || allNear || allFar;
}

[shader("compute")]
[numthreads(64, 1, 1)]
void csMain(uint3 tid : SV_DispatchThreadID) {
    uint index = tid.x;
    uint threadCapacity = runtime.instances / 64 + 1;
    uint offset = index * threadCapacity;

    uint upper = min(runtime.instances, offset + threadCapacity);
    for (uint i = offset; i < upper; i++) {
        if (!Culling(i)) {
            uint myIndex;
            InterlockedAdd(count[0], 1, myIndex);
            indices[myIndex] = i;
        }
    }

    AllMemoryBarrierWithGroupSync();

    if (index == 0) {
        IndirectCommand cmd;
        cmd.indexCount = 36;
        cmd.instanceCount = count[0];
        cmd.firstInstance = 0;
        cmd.firstIndex = 0;
        cmd.offset = 0;
        indirect[0] = cmd;
    }
}