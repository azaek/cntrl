import { onCleanup, onMount } from "solid-js";
import type { JSX } from "solid-js/jsx-runtime";

type PixelBlastVariant = "square" | "circle" | "triangle" | "diamond";

type PixelBlastProps = {
  variant?: PixelBlastVariant;
  pixelSize?: number;
  color?: string;
  class?: string;
  style?: JSX.CSSProperties;
  patternScale?: number;
  patternDensity?: number;
  pixelSizeJitter?: number;
  autoPauseOffscreen?: boolean;
  speed?: number;
  edgeFade?: number;
};

const SHAPE_MAP: Record<PixelBlastVariant, number> = {
  square: 0,
  circle: 1,
  triangle: 2,
  diamond: 3,
};

const VERT = `#version 300 es
in vec2 a_position;
void main() {
  gl_Position = vec4(a_position, 0.0, 1.0);
}
`;

const FRAG = `#version 300 es
precision highp float;

uniform vec3  uColor;
uniform vec2  uResolution;
uniform float uTime;
uniform float uPixelSize;
uniform float uScale;
uniform float uDensity;
uniform float uPixelJitter;
uniform float uEdgeFade;
uniform int   uShapeType;

out vec4 fragColor;

float Bayer2(vec2 a) {
  a = floor(a);
  return fract(a.x / 2.0 + a.y * a.y * 0.75);
}
#define Bayer4(a) (Bayer2(0.5*(a))*0.25 + Bayer2(a))
#define Bayer8(a) (Bayer4(0.5*(a))*0.25 + Bayer2(a))

#define FBM_OCTAVES    5
#define FBM_LACUNARITY 1.25
#define FBM_GAIN       1.0

float hash11(float n) { return fract(sin(n) * 43758.5453); }

float vnoise(vec3 p) {
  vec3 ip = floor(p);
  vec3 fp = fract(p);
  float n000 = hash11(dot(ip + vec3(0,0,0), vec3(1,57,113)));
  float n100 = hash11(dot(ip + vec3(1,0,0), vec3(1,57,113)));
  float n010 = hash11(dot(ip + vec3(0,1,0), vec3(1,57,113)));
  float n110 = hash11(dot(ip + vec3(1,1,0), vec3(1,57,113)));
  float n001 = hash11(dot(ip + vec3(0,0,1), vec3(1,57,113)));
  float n101 = hash11(dot(ip + vec3(1,0,1), vec3(1,57,113)));
  float n011 = hash11(dot(ip + vec3(0,1,1), vec3(1,57,113)));
  float n111 = hash11(dot(ip + vec3(1,1,1), vec3(1,57,113)));
  vec3 w = fp*fp*fp*(fp*(fp*6.0-15.0)+10.0);
  float x00 = mix(n000, n100, w.x);
  float x10 = mix(n010, n110, w.x);
  float x01 = mix(n001, n101, w.x);
  float x11 = mix(n011, n111, w.x);
  float y0  = mix(x00, x10, w.y);
  float y1  = mix(x01, x11, w.y);
  return mix(y0, y1, w.z) * 2.0 - 1.0;
}

float fbm2(vec2 uv, float t) {
  vec3 p = vec3(uv * uScale, t);
  float amp = 1.0, freq = 1.0, sum = 1.0;
  for (int i = 0; i < FBM_OCTAVES; ++i) {
    sum  += amp * vnoise(p * freq);
    freq *= FBM_LACUNARITY;
    amp  *= FBM_GAIN;
  }
  return sum * 0.5 + 0.5;
}

float maskCircle(vec2 p, float cov) {
  float r = sqrt(cov) * 0.25;
  float d = length(p - 0.5) - r;
  float aa = 0.5 * fwidth(d);
  return cov * (1.0 - smoothstep(-aa, aa, d * 2.0));
}

float maskTriangle(vec2 p, vec2 id, float cov) {
  bool flip = mod(id.x + id.y, 2.0) > 0.5;
  if (flip) p.x = 1.0 - p.x;
  float r = sqrt(cov);
  float d  = p.y - r * (1.0 - p.x);
  float aa = fwidth(d);
  return cov * clamp(0.5 - d / aa, 0.0, 1.0);
}

float maskDiamond(vec2 p, float cov) {
  float r = sqrt(cov) * 0.564;
  return step(abs(p.x - 0.49) + abs(p.y - 0.49), r);
}

void main() {
  vec2 fragCoord = gl_FragCoord.xy - uResolution * 0.5;
  float aspect = uResolution.x / uResolution.y;

  vec2 pixelId = floor(fragCoord / uPixelSize);
  vec2 pixelUV = fract(fragCoord / uPixelSize);

  float cellPixelSize = 8.0 * uPixelSize;
  vec2 cellId = floor(fragCoord / cellPixelSize);
  vec2 cellCoord = cellId * cellPixelSize;
  vec2 uv = cellCoord / uResolution * vec2(aspect, 1.0);

  float base = fbm2(uv, uTime * 0.05);
  base = base * 0.5 - 0.65;
  float feed = base + (uDensity - 0.5) * 0.3;

  float bayer = Bayer8(fragCoord / uPixelSize) - 0.5;
  float bw = step(0.5, feed + bayer);

  float h = fract(sin(dot(floor(fragCoord / uPixelSize), vec2(127.1, 311.7))) * 43758.5453);
  float jitterScale = 1.0 + (h - 0.5) * uPixelJitter;
  float coverage = bw * jitterScale;

  float M;
  if      (uShapeType == 1) M = maskCircle(pixelUV, coverage);
  else if (uShapeType == 2) M = maskTriangle(pixelUV, pixelId, coverage);
  else if (uShapeType == 3) M = maskDiamond(pixelUV, coverage);
  else                      M = coverage;

  if (uEdgeFade > 0.0) {
    vec2 norm = gl_FragCoord.xy / uResolution;
    float edge = min(min(norm.x, norm.y), min(1.0 - norm.x, 1.0 - norm.y));
    M *= smoothstep(0.0, uEdgeFade, edge);
  }

  vec3 srgb = mix(
    uColor * 12.92,
    1.055 * pow(uColor, vec3(1.0 / 2.4)) - 0.055,
    step(0.0031308, uColor)
  );

  fragColor = vec4(srgb, M);
}
`;

// ── WebGL helpers ────────────────────────────────────────────────────

function compileShader(gl: WebGL2RenderingContext, type: number, src: string) {
  const s = gl.createShader(type)!;
  gl.shaderSource(s, src);
  gl.compileShader(s);
  if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) {
    const log = gl.getShaderInfoLog(s);
    gl.deleteShader(s);
    throw new Error(`Shader compile error: ${log}`);
  }
  return s;
}

function createProgram(gl: WebGL2RenderingContext) {
  const vs = compileShader(gl, gl.VERTEX_SHADER, VERT);
  const fs = compileShader(gl, gl.FRAGMENT_SHADER, FRAG);
  const prog = gl.createProgram()!;
  gl.attachShader(prog, vs);
  gl.attachShader(prog, fs);
  gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
    const log = gl.getProgramInfoLog(prog);
    gl.deleteProgram(prog);
    throw new Error(`Program link error: ${log}`);
  }
  gl.deleteShader(vs);
  gl.deleteShader(fs);
  return prog;
}

function hexToLinearRGB(hex: string): [number, number, number] {
  let c = hex.replace("#", "");
  if (c.length === 3) c = c[0] + c[0] + c[1] + c[1] + c[2] + c[2];
  const r = parseInt(c.slice(0, 2), 16) / 255;
  const g = parseInt(c.slice(2, 4), 16) / 255;
  const b = parseInt(c.slice(4, 6), 16) / 255;
  const toLinear = (v: number) =>
    v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
  return [toLinear(r), toLinear(g), toLinear(b)];
}

// ── component ────────────────────────────────────────────────────────

export default function PixelBlast(props: PixelBlastProps) {
  let containerEl!: HTMLDivElement;

  onMount(() => {
    const variant = props.variant ?? "square";
    const pixelSize = props.pixelSize ?? 3;
    const color = props.color ?? "#B19EEF";
    const patternScale = props.patternScale ?? 2;
    const patternDensity = props.patternDensity ?? 1;
    const pixelSizeJitter = props.pixelSizeJitter ?? 0;
    const autoPause = props.autoPauseOffscreen ?? true;
    const speed = props.speed ?? 0.5;
    const edgeFade = props.edgeFade ?? 0.5;

    const canvas = document.createElement("canvas");
    canvas.style.width = "100%";
    canvas.style.height = "100%";
    canvas.style.display = "block";
    containerEl.appendChild(canvas);

    const gl = canvas.getContext("webgl2", {
      alpha: true,
      antialias: false,
      premultipliedAlpha: false,
      powerPreference: "low-power",
    });
    if (!gl) {
      console.warn("WebGL2 not available");
      return;
    }

    const prog = createProgram(gl);
    gl.useProgram(prog);

    // fullscreen triangle — more efficient than a quad (3 verts, no index buffer)
    const vao = gl.createVertexArray()!;
    gl.bindVertexArray(vao);
    const buf = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(
      gl.ARRAY_BUFFER,
      new Float32Array([-1, -1, 3, -1, -1, 3]),
      gl.STATIC_DRAW,
    );
    const aPos = gl.getAttribLocation(prog, "a_position");
    gl.enableVertexAttribArray(aPos);
    gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0);

    // uniform locations
    const loc = (name: string) => gl.getUniformLocation(prog, name);
    const locs = {
      uColor: loc("uColor"),
      uResolution: loc("uResolution"),
      uTime: loc("uTime"),
      uPixelSize: loc("uPixelSize"),
      uScale: loc("uScale"),
      uDensity: loc("uDensity"),
      uPixelJitter: loc("uPixelJitter"),
      uEdgeFade: loc("uEdgeFade"),
      uShapeType: loc("uShapeType"),
    };

    const timeOffset =
      (crypto.getRandomValues(new Uint32Array(1))[0] / 0xffffffff) * 1000;
    const startTime = performance.now();

    gl.enable(gl.BLEND);
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);

    // sizing
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const resize = () => {
      const w = containerEl.clientWidth || 1;
      const h = containerEl.clientHeight || 1;
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
      gl.viewport(0, 0, canvas.width, canvas.height);
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(containerEl);

    // visibility for auto-pause
    let visible = true;
    let io: IntersectionObserver | undefined;
    if (autoPause) {
      io = new IntersectionObserver(
        ([entry]) => {
          visible = entry.isIntersecting;
        },
        { threshold: 0 },
      );
      io.observe(containerEl);
    }

    const [r, g, b] = hexToLinearRGB(color);

    let raf = 0;
    const animate = () => {
      raf = requestAnimationFrame(animate);
      if (autoPause && !visible) return;

      const elapsed = (performance.now() - startTime) / 1000;
      const t = timeOffset + elapsed * speed;

      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);

      gl.useProgram(prog);
      gl.uniform3f(locs.uColor, r, g, b);
      gl.uniform2f(locs.uResolution, canvas.width, canvas.height);
      gl.uniform1f(locs.uTime, t);
      gl.uniform1f(locs.uPixelSize, pixelSize * dpr);
      gl.uniform1f(locs.uScale, patternScale);
      gl.uniform1f(locs.uDensity, patternDensity);
      gl.uniform1f(locs.uPixelJitter, pixelSizeJitter);
      gl.uniform1f(locs.uEdgeFade, edgeFade);
      gl.uniform1i(locs.uShapeType, SHAPE_MAP[variant] ?? 0);

      gl.bindVertexArray(vao);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    };
    raf = requestAnimationFrame(animate);

    onCleanup(() => {
      cancelAnimationFrame(raf);
      ro.disconnect();
      io?.disconnect();
      gl.deleteBuffer(buf);
      gl.deleteVertexArray(vao);
      gl.deleteProgram(prog);
      if (canvas.parentElement === containerEl) containerEl.removeChild(canvas);
    });
  });

  return (
    <div
      ref={containerEl!}
      class={props.class}
      style={{
        width: "100%",
        height: "100%",
        overflow: "hidden",
        ...props.style,
      }}
      aria-label="Animated pixel background"
    />
  );
}
