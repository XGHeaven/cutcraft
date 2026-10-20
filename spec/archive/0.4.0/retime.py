"""Exact reference time mapping and new retime constraints; no media decoding."""
from fractions import Fraction
from math import gcd


def rational(v):
    n, d = v['num'], v['den']
    if d <= 0 or gcd(n, d) != 1:
        raise ValueError('noncanonical rational')
    return Fraction(n, d)


def segment(a, b):
    d = rational(b['offset']) - rational(a['offset'])
    s0, s1 = rational(a['value']), rational(b['value'])
    easing = a.get('easing', {'type': 'linear'})
    if easing['type'] == 'linear':
        m0 = m1 = (s1 - s0) / d
    else:
        m0, m1 = rational(easing['outRate']), rational(easing['inRate'])
    return d, s0, s1, m0, m1


def coefficients(a, b):
    d, s0, s1, m0, m1 = segment(a, b)
    return 2*s0-2*s1+d*(m0+m1), -3*s0+3*s1-d*(2*m0+m1), d*m0, s0


def evaluate(time_map, tau):
    frames = time_map['$animation']['keyframes']
    if tau < rational(frames[0]['offset']) or tau > rational(frames[-1]['offset']):
        raise ValueError('timeMap evaluation outside its domain; handle extension is explicit')
    if tau == rational(frames[-1]['offset']):
        return rational(frames[-1]['value'])
    for a, b in zip(frames, frames[1:]):
        t0, t1 = rational(a['offset']), rational(b['offset'])
        if t0 <= tau < t1:
            u = (tau - t0)/(t1-t0)
            A, B, C, D = coefficients(a, b)
            return ((A*u+B)*u+C)*u+D
    raise ValueError('missing timeMap segment')


def validate_item(item, stream_kind, validate_animation):
    playback = item.get('playback', {})
    mode = playback.get('mode', 'fit')
    duration = rational(item['timelineRange']['duration'])
    audio = stream_kind == 'audio'
    if mode == 'hold' and audio:
        raise ValueError('audio cannot use hold')
    if mode == 'remap' and stream_kind not in ['video', 'audio', 'sequence']:
        raise ValueError('remap requires a timed source')
    if audio and 'videoSampling' in playback:
        raise ValueError('videoSampling does not apply to audio')
    if not audio and 'audioPolicy' in playback:
        raise ValueError('audioPolicy belongs to audio items')
    identity = mode == 'loop' and not playback.get('reverse', False)
    negative = playback.get('reverse', False)
    if 'sourceRange' in item:
        length = rational(item['sourceRange']['duration'])
        if mode == 'fit':
            identity = length == duration and not negative
    if mode == 'remap':
        length = rational(item['sourceRange']['duration'])
        mapping = playback['timeMap']
        validate_animation(mapping, duration)
        frames = mapping['$animation']['keyframes']
        if len(frames) < 2 or rational(frames[0]['offset']) != 0 or rational(frames[-1]['offset']) != duration:
            raise ValueError('timeMap must cover [0, duration] with at least two keys')
        identity = True
        negative = False
        for a, b in zip(frames, frames[1:]):
            d, s0, s1, m0, m1 = segment(a, b)
            if not all(0 <= value <= length for value in [s0, s0+d*m0/3, s1-d*m1/3, s1]):
                raise ValueError('timeMap control hull exceeds sourceRange')
            A, B, C, _ = coefficients(a, b)
            identity &= A == 0 and B == 0 and C == d
            # Minimum derivative on [0,1], exactly, including internal reversals.
            derivatives = [C, 3*A+2*B+C]
            if A > 0:
                u = -B/(3*A)
                if 0 < u < 1:
                    derivatives.append(3*A*u*u+2*B*u+C)
            negative |= min(derivatives) < 0
    if audio and not identity and 'audioPolicy' not in playback:
        raise ValueError('retimed audio requires an explicit audioPolicy')
    if audio and playback.get('audioPolicy', {}).get('mode') == 'preserve-pitch' and negative:
        raise ValueError('preserve-pitch does not support reverse intervals')
