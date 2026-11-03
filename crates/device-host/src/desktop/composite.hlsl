// Rotate and composite DXGI cursor shapes without reading the desktop back to RAM.
Texture2D<float4> desktop : register(t0);
ByteAddressBuffer cursor : register(t1);
RWTexture2D<float4> output_image : register(u0);
SamplerState linear_clamp : register(s0);
cbuffer Geometry : register(b0) {
    uint width, height, out_width, out_height;
    uint rotation; int pointer_x, pointer_y; uint visible;
    uint shape_type, shape_width, shape_height, pitch;
};
uint byte_at(uint n) { return (cursor.Load(n & ~3u) >> ((n & 3u) * 8u)) & 255u; }
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= out_width || id.y >= out_height) return;
    float2 uv = (float2(id.xy) + 0.5) / float2(out_width, out_height);
    float2 source = uv;
    if (rotation == 2) source = float2(uv.y, 1.0 - uv.x);
    else if (rotation == 3) source = 1.0 - uv;
    else if (rotation == 4) source = float2(1.0 - uv.y, uv.x);
    float3 color = desktop.SampleLevel(linear_clamp, source, 0).rgb;
    int2 p = int2(uv * float2(width, height)) - int2(pointer_x, pointer_y);
    if (visible && p.x >= 0 && p.y >= 0 && p.x < (int)shape_width && p.y < (int)shape_height) {
        uint3 base = (uint3)round(saturate(color) * 255.0);
        if (shape_type == 1) { // monochrome: separate AND and XOR bit planes
            uint mask = 0x80u >> (p.x & 7);
            uint a = (byte_at(p.y * pitch + p.x / 8) & mask) ? 255 : 0;
            uint x = (byte_at((p.y + shape_height) * pitch + p.x / 8) & mask) ? 255 : 0;
            color = float3((base & a) ^ x) / 255.0;
        } else {
            uint n = p.y * pitch + p.x * 4;
            uint3 c = uint3(byte_at(n+2), byte_at(n+1), byte_at(n));
            uint alpha = byte_at(n+3);
            if (shape_type == 4) color = float3(alpha ? (base ^ c) : c) / 255.0;
            else color = float3((c * alpha + base * (255-alpha)) / 255) / 255.0;
        }
    }
    output_image[id.xy] = float4(color, 1);
}
