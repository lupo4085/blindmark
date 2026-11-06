"""Simulated 'photo of a screen' (camera recapture) for evaluating blindmark.

Pipeline: image shown on an LCD (RGB sub-pixel stripes + pixel gaps, dark bezel)
-> camera pose (tilt / roll / distance) as a perspective projection, sampled directly
   from the sub-pixel render so real aliasing / moiré appears
-> lens barrel distortion -> optics + defocus blur -> vignetting -> exposure / white balance
-> sensor noise -> phone sharpening -> JPEG.
"""
from __future__ import annotations

import numpy as np
import cv2

from attacks import jpeg


def _screen_render(img: np.ndarray, display_w: int, rng) -> np.ndarray:
    """Image as shown on a screen, rendered at 3x3 sub-samples per screen pixel."""
    h, w = img.shape[:2]
    dh = max(8, round(h * display_w / w))
    scr = cv2.resize(img, (display_w, dh), interpolation=cv2.INTER_AREA if display_w < w else cv2.INTER_CUBIC)
    # dark UI margin around the picture (browser / viewer chrome)
    m = int(display_w * 0.06)
    canvas = np.full((dh + 2 * m, display_w + 2 * m, 3), int(rng.integers(15, 40)), np.uint8)
    canvas[m:m + dh, m:m + display_w] = scr
    up = np.repeat(np.repeat(canvas.astype(np.float32), 3, axis=0), 3, axis=1)
    mask = np.zeros((3, 3, 3), np.float32)              # RGB stripes, 1 dark row (pixel gap)
    for c in range(3):
        mask[:2, c, c] = 1.0
    mask[2] *= 0.15
    tiled = np.tile(mask, (canvas.shape[0], canvas.shape[1], 1))
    return up * tiled * 4.5                                # 2 of 9 sub-cells lit per channel


def screen_photo(img: np.ndarray, rng, tilt=(0, 5), roll=(-5, 5), fill=(0.55, 0.8),
                 blur=(0.6, 1.2), noise=(1.5, 3.5), q=(85, 93), cam_w: int = 1600,
                 return_meta: bool = False):
    cam_h = cam_w * 3 // 4
    screen = _screen_render(img, display_w=int(rng.integers(800, 1100)), rng=rng)
    sh, sw = screen.shape[:2]
    # camera pose: yaw/pitch tilt of the screen plane + in-plane roll, pinhole projection
    t_yaw = np.deg2rad(rng.uniform(*tilt)) * rng.choice([-1, 1])
    t_pitch = np.deg2rad(rng.uniform(*tilt)) * rng.choice([-1, 1]) * 0.5
    r = np.deg2rad(rng.uniform(*roll))
    Ry = np.array([[np.cos(t_yaw), 0, np.sin(t_yaw)], [0, 1, 0], [-np.sin(t_yaw), 0, np.cos(t_yaw)]])
    Rx = np.array([[1, 0, 0], [0, np.cos(t_pitch), -np.sin(t_pitch)], [0, np.sin(t_pitch), np.cos(t_pitch)]])
    corners = np.array([[-sw / 2, -sh / 2, 0], [sw / 2, -sh / 2, 0], [sw / 2, sh / 2, 0], [-sw / 2, sh / 2, 0]])
    P = corners @ (Ry @ Rx).T
    f = 1.2 * sw                                           # focal length ~ normal lens
    P[:, 2] += f * 1.0
    proj = P[:, :2] * (f / P[:, 2:3])
    c, s = np.cos(r), np.sin(r)
    proj = proj @ np.array([[c, -s], [s, c]]).T
    span = proj.max(0) - proj.min(0)
    k = rng.uniform(*fill) * min(cam_w / span[0], cam_h / span[1])
    proj = (proj - proj.mean(0)) * k + np.array([cam_w / 2, cam_h / 2]) + rng.uniform(-0.05, 0.05, 2) * [cam_w, cam_h]
    src = np.float32([[0, 0], [sw, 0], [sw, sh], [0, sh]])
    H = cv2.getPerspectiveTransform(src, np.float32(proj))
    # optics: slight blur at sub-pixel resolution before sampling (does NOT remove all aliasing)
    scale = np.sqrt(abs(np.linalg.det(H[:2, :2])))
    pre = cv2.GaussianBlur(screen, (0, 0), max(0.3, 0.35 / max(scale, 1e-3)))
    room = np.full((cam_h, cam_w, 3), rng.uniform(25, 70), np.float32)
    shot = cv2.warpPerspective(pre, H, (cam_w, cam_h), flags=cv2.INTER_LINEAR,
                               borderMode=cv2.BORDER_TRANSPARENT, dst=room)
    # barrel distortion
    k1 = rng.uniform(0.0, 0.04)
    yy, xx = np.mgrid[0:cam_h, 0:cam_w].astype(np.float32)
    nx, ny = (xx - cam_w / 2) / (cam_w / 2), (yy - cam_h / 2) / (cam_w / 2)
    rr = nx * nx + ny * ny
    mapx = (nx * (1 + k1 * rr)) * (cam_w / 2) + cam_w / 2
    mapy = (ny * (1 + k1 * rr)) * (cam_w / 2) + cam_h / 2
    shot = cv2.remap(shot, mapx, mapy, cv2.INTER_LINEAR, borderMode=cv2.BORDER_REFLECT)
    shot = cv2.GaussianBlur(shot, (0, 0), rng.uniform(*blur))            # defocus
    vig = 1.0 - rng.uniform(0.1, 0.3) * rr
    gain = rng.uniform(0.85, 1.15) * rng.uniform(0.92, 1.08, 3)           # exposure + white balance
    shot = shot * vig[..., None] * gain
    shot = 255.0 * np.clip(shot / 255.0, 0, 1) ** rng.uniform(0.85, 1.1)
    sig = rng.uniform(*noise)
    shot = shot + rng.normal(0, 1, shot.shape) * (sig * np.sqrt(np.maximum(shot, 1) / 128.0))
    shot = np.clip(shot, 0, 255).astype(np.uint8)
    blur2 = cv2.GaussianBlur(shot, (0, 0), 1.0)                            # phone sharpening
    shot = cv2.addWeighted(shot, 1.4, blur2, -0.4, 0)
    out = jpeg(shot, int(rng.integers(*q)))
    if return_meta:
        return out, {"tilt_deg": float(np.rad2deg(abs(t_yaw))), "roll_deg": float(np.rad2deg(r))}
    return out


LEVELS = {
    "photo_frontal": dict(tilt=(0, 4), roll=(-4, 4)),
    "photo_roll30": dict(tilt=(0, 4), roll=(-30, 30)),
    "photo_tilt10": dict(tilt=(8, 12), roll=(-6, 6)),
    "photo_tilt20": dict(tilt=(18, 22), roll=(-6, 6)),
    "photo_tilt30": dict(tilt=(28, 32), roll=(-6, 6)),
    "photo_far": dict(tilt=(0, 6), roll=(-6, 6), fill=(0.3, 0.4)),
}
