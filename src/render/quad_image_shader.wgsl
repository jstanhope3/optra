// fullscreen_quad.wgsl
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

struct DataBlock {
    row1: vec4<f32>,
    row2: vec4<f32>,
    row3: vec4<f32>,
    row4: vec4<f32>,
}



@vertex
fn vs_main(
    model: VertexInput,
) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4(model.position, 0.0, 1.0);
    out.uv = model.uv;
    return out;
}

@group(0) @binding(0)
var my_tex: texture_2d<f32>;
@group(0) @binding(1)
var my_sampler: sampler;
@group(0) @binding(2)
var<uniform> data_block: DataBlock;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {

    // Checkerboard colours come from the active theme; see `ThemeChoice`.
    let color_a = vec3(data_block.row3[2], data_block.row3[3], data_block.row4[0]);
    let color_b = vec3(data_block.row4[1], data_block.row4[2], data_block.row4[3]);

    let x = in.uv.x;
    let y = in.uv.y;

    // View transform: zoom about the container centre, then pan (both in container-uv units).
    let zoom: f32 = max(data_block.row2[0], 0.0001);
    let pan_x: f32 = data_block.row2[1];
    let pan_y: f32 = data_block.row2[2];

    // HDR (linear float) sources need exposure + gamma encoding; 8-bit sources are
    // already display-encoded and are passed through untouched.
    let is_hdr: bool = data_block.row2[3] > 0.5;
    let exposure: f32 = data_block.row3[0];
    let gamma: f32 = max(data_block.row3[1], 0.0001);

    let vx = (x - pan_x - 0.5) / zoom + 0.5;
    let vy = (y - pan_y - 0.5) / zoom + 0.5;

    let img_width: f32 = data_block.row1[0];
    let img_height: f32 = data_block.row1[1];
    let container_width: f32 = data_block.row1[2];
    let container_height: f32 = data_block.row1[3];

    let container_aspect_ratio = container_width / container_height;

    var display_width = 0.0;
    var display_height = 0.0;

    // Fit-to-contain: compare the image's aspect against the CONTAINER's.
    // Testing `img_width > img_height` instead ignores the container shape, so
    // resizing the side panel changed how the image was fitted.
    let img_aspect = img_width / img_height;

    if img_aspect > container_aspect_ratio {
        display_width = container_width;
        display_height = container_width / img_aspect;
    } else {
        display_height = container_height;
        display_width = container_height * img_aspect;
    }

    let diff_width = container_width - display_width;
    let diff_height = container_height - display_height;

    let x_offset = max(diff_width / (2 * container_width), 0.0);
    let y_offset = max(diff_height / (2 * container_height), 0.0);

    let m_x = display_width / container_width;
    let m_y = display_height / container_height;

    let precomputed_tex_coords = vec2((vx - x_offset) / m_x, (vy - y_offset) / m_y);
    let background_color = vec4(0.0);

    if precomputed_tex_coords.x > 1.0 || precomputed_tex_coords.x < 0.0 || precomputed_tex_coords.y > 1.0 || precomputed_tex_coords.y < 0.0 {
        let c_x = floor(20.0 * x) / 2.0;
        let c_y = floor(20.0 * y / container_aspect_ratio) / 2.0;
        if fract(c_x) == fract(c_y) {
            return vec4(color_a, 1.0);
        } else {
            return vec4(color_b, 1.0);
        }
    }

    let sampled = textureSample(my_tex, my_sampler, precomputed_tex_coords);

    if !is_hdr {
        return sampled;
    }

    // Exposure in stops, then gamma encode. Negatives are clamped so the
    // pow() below never sees a NaN-producing input.
    let exposed = max(sampled.rgb * exp2(exposure), vec3(0.0));
    let encoded = pow(exposed, vec3(1.0 / gamma));

    return vec4(clamp(encoded, vec3(0.0), vec3(1.0)), sampled.a);
}
