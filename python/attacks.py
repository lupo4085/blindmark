"""Screenshot / sharing distortions used to evaluate blindmark."""
from __future__ import annotations

import cv2
import numpy as np


def resize(img, s):
    h, w = img.shape[:2]
    interp = cv2.INTER_AREA if s < 1 else cv2.INTER_CUBIC
    return cv2.resize(img, (max(1, round(w * s)), max(1, round(h * s))), interpolation=interp)


def crop(img, area, rng):
    h, w = img.shape[:2]
    f = np.sqrt(area)
    ch, cw = max(8, int(h * f)), max(8, int(w * f))
    y = rng.integers(0, h - ch + 1); x = rng.integers(0, w - cw + 1)
    return img[y:y + ch, x:x + cw].copy()


def jpeg(img, q):
    ok, buf = cv2.imencode(".jpg", cv2.cvtColor(img, cv2.COLOR_RGB2BGR), [cv2.IMWRITE_JPEG_QUALITY, int(q)])
    return cv2.cvtColor(cv2.imdecode(buf, cv2.IMREAD_COLOR), cv2.COLOR_BGR2RGB)


def webp(img, q):
    ok, buf = cv2.imencode(".webp", cv2.cvtColor(img, cv2.COLOR_RGB2BGR), [cv2.IMWRITE_WEBP_QUALITY, int(q)])
    return cv2.cvtColor(cv2.imdecode(buf, cv2.IMREAD_COLOR), cv2.COLOR_BGR2RGB)


def gamma(img, g):
    x = img.astype(np.float64) / 255.0
    return np.clip(np.round(255 * x ** g), 0, 255).astype(np.uint8)


def ui_frame(img, rng):
    """Paste the image into a fake app window: bars, margins, text, offset."""
    h, w = img.shape[:2]
    top, bot = int(rng.integers(40, 90)), int(rng.integers(30, 80))
    left, right = int(rng.integers(10, 120)), int(rng.integers(10, 120))
    bg = int(rng.choice([255, 245, 30, 18]))
    canvas = np.full((h + top + bot, w + left + right, 3), bg, np.uint8)
    canvas[:top - 8] = (rng.integers(40, 220, 3)).astype(np.uint8)
    fg = (0, 0, 0) if bg > 128 else (230, 230, 230)
    cv2.putText(canvas, "12:41  Photos - shared album", (8, top - 20), cv2.FONT_HERSHEY_SIMPLEX, 0.6, fg, 1, cv2.LINE_AA)
    cv2.putText(canvas, "like  comment  share", (8, h + top + bot - 10), cv2.FONT_HERSHEY_SIMPLEX, 0.6, fg, 1, cv2.LINE_AA)
    canvas[top:top + h, left:left + w] = img
    return canvas


def screenshot(img, rng, scale_range=(0.45, 1.0), area_range=(0.4, 0.9), q_range=(65, 90)):
    """Display at some zoom -> partial capture with UI -> gamma -> re-share as JPEG."""
    x = resize(img, rng.uniform(*scale_range))
    x = crop(x, rng.uniform(*area_range), rng)
    x = ui_frame(x, rng)
    x = gamma(x, rng.uniform(0.85, 1.15))
    return jpeg(x, rng.integers(*q_range))


def suite(rng):
    """name -> function(img) for the benchmark."""
    return {
        "none": lambda x: x,
        "jpeg_q75": lambda x: jpeg(x, 75),
        "jpeg_q50": lambda x: jpeg(x, 50),
        "webp_q70": lambda x: webp(x, 70),
        "scale_0.5": lambda x: resize(x, 0.5),
        "scale_0.75": lambda x: resize(x, 0.75),
        "scale_1.5": lambda x: resize(x, 1.5),
        "crop_50%": lambda x: crop(x, 0.5, rng),
        "crop_25%": lambda x: crop(x, 0.25, rng),
        "ui_frame": lambda x: ui_frame(x, rng),
        "gamma_0.8": lambda x: gamma(x, 0.8),
        "screenshot_mild": lambda x: screenshot(x, rng, (0.75, 1.0), (0.7, 0.95), (80, 92)),
        "screenshot_typical": lambda x: screenshot(x, rng),
        "screenshot_hard": lambda x: screenshot(x, rng, (0.35, 0.6), (0.3, 0.6), (55, 75)),
    }
