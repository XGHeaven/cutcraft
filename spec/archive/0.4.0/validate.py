#!/usr/bin/env python3
"""Draft schema + animation/transition semantics; not a renderer/media validator."""
import copy
import json
import math
import retime
from fractions import Fraction
from pathlib import Path
from jsonschema import Draft202012Validator, FormatChecker
from jsonschema.exceptions import ValidationError
from referencing import Registry, Resource

ROOT = Path(__file__).resolve().parent
schemas = {p.name: json.loads(p.read_text()) for p in (ROOT / 'schemas').glob('*.json')}
for schema in schemas.values():
    Draft202012Validator.check_schema(schema)
registry = Registry().with_resources((s['$id'], Resource.from_contents(s)) for s in schemas.values())
validator = Draft202012Validator(schemas['project.schema.json'], registry=registry, format_checker=FormatChecker())


def time(value):
    n, d = value['num'], value['den']
    if d <= 0 or math.gcd(n, d) != 1:
        raise ValueError('noncanonical rational time')
    return Fraction(n, d)


def animation(value, duration, progress=False):
    if not isinstance(value, dict) or '$animation' not in value:
        return
    data = value['$animation']
    frames = data['keyframes']
    offsets = [time(k['offset']) for k in frames]
    if any(t < 0 or t > duration for t in offsets):
        raise ValueError('animation offset outside owner duration')
    if any(a >= b for a, b in zip(offsets, offsets[1:])):
        raise ValueError('keyframes must strictly increase')
    if 'easing' in frames[-1]:
        raise ValueError('last keyframe must omit easing')
    for k in frames[:-1]:
        easing = k.get('easing', {})
        if easing.get('type') == 'cubic-bezier' and easing['x1'] > easing['x2']:
            raise ValueError('bezier requires x1 <= x2')
    if progress and (offsets[0] != 0 or offsets[-1] != duration or frames[0]['value'] != 0 or frames[-1]['value'] != 1):
        raise ValueError('transition progress requires 0->1 over the complete window')


def semantics(project):
    # Only registered slots are evaluated; opaque plugin JSON is not traversed.
    for seq in project['sequences'].values():
        locations = {}
        sequence_end = max((time(i['timelineRange']['start']) + time(i['timelineRange']['duration']) for t in seq['tracks'] for i in t['items']), default=Fraction(0))
        if seq['durationMode'] == 'fixed':
            sequence_end = time(seq['duration'])
        for track in seq['tracks']:
            for item in track['items']:
                duration = time(item['timelineRange']['duration'])
                locations[item['id']] = (track, item)
                if item['type'] in ['media', 'sequence']:
                    if item['type'] == 'media':
                        asset = project['assets'][item['assetId']]
                        stream = next(stream for stream in asset['streams'] if stream['id'] == item['streamId'])
                        source_kind = stream['kind']
                    else:
                        source_kind = 'audio' if track['kind'] == 'audio' else 'sequence'
                    retime.validate_item(item, source_kind, animation)
                video, audio = item.get('video', {}), item.get('audio', {})
                transform = video.get('transform', {})
                for name in ['position', 'anchor', 'scale']:
                    value = transform.get(name, {})
                    if '$animation' in value:
                        animation(value, duration)
                    else:
                        for field in ['x', 'y']:
                            animation(value.get(field), duration)
                animation(transform.get('rotationDegrees'), duration)
                animation(video.get('opacity'), duration)
                for field in ['top', 'right', 'bottom', 'left']:
                    animation(transform.get('crop', {}).get(field), duration)
                for field in ['gainDb', 'pan']:
                    animation(audio.get(field), duration)
                for effect in item.get('effects', []):
                    if effect.get('version') == '2.0.0':
                        kind = effect['effectType']
                        fields = ['exposureEv', 'temperature', 'tint', 'contrast', 'pivot', 'saturation'] if kind == 'builtin.color-grade' else ['strength'] if kind == 'builtin.lut' else []
                        for field in fields:
                            animation(effect['parameters'].get(field), duration)
        windows = {}
        for transition in seq.get('transitions', []):
            if transition['transitionType'] not in ['builtin.crossfade', 'builtin.dip-to-color']:
                continue  # Plugin-defined semantics require that plugin.
            at, a = locations[transition['fromItemId']]
            bt, b = locations[transition['toItemId']]
            if at['id'] != bt['id'] or at['kind'] != 'video':
                raise ValueError('built-in transitions require the same video track')
            start_a = time(a['timelineRange']['start'])
            c = start_a + time(a['timelineRange']['duration'])
            if c != time(b['timelineRange']['start']):
                raise ValueError('transition items must be adjacent')
            d = time(transition['duration'])
            shift = {'before': d, 'center': d / 2, 'after': Fraction(0)}[transition['alignment']]
            start, end = c - shift, c - shift + d
            end_b = time(b['timelineRange']['start']) + time(b['timelineRange']['duration'])
            if start < max(Fraction(0), start_a) or end > min(sequence_end, end_b):
                raise ValueError('transition window exceeds sequence/items')
            for lo, hi in windows.setdefault(at['id'], []):
                if start < hi and lo < end:
                    raise ValueError('transition windows overlap')
            windows[at['id']].append((start, end))
            if 'progress' in transition:
                animation(transition['progress'], d, progress=True)


def validate(project):
    json.dumps(project, allow_nan=False)
    validator.validate(project)
    semantics(project)


def reject(name, project):
    try:
        validate(project)
    except (ValueError, ValidationError) as error:
        # Report a failure only after a schema/semantic exception was observed.
        print('PASS reject:', name, '-', type(error).__name__)
    else:
        raise AssertionError('incorrectly accepted ' + name)


def evaluate(envelope, tau):
    """Reference value evaluation only; does not integrate with the CLI."""
    track = envelope['$animation']
    frames = track['keyframes']
    kind = track['valueType']
    def normalized(value):
        if kind == 'color' and value['a'] == 0:
            return {'r':0, 'g':0, 'b':0, 'a':0}
        return copy.deepcopy(value)
    for key in frames:
        if tau == time(key['offset']):
            return normalized(key['value'])
    if tau <= time(frames[0]['offset']):
        return normalized(frames[0]['value'])
    if tau >= time(frames[-1]['offset']):
        return normalized(frames[-1]['value'])
    if kind == 'time':
        result = retime.evaluate(envelope, tau)
        return {'num': result.numerator, 'den': result.denominator}
    for a, b in zip(frames, frames[1:]):
        t0, t1 = time(a['offset']), time(b['offset'])
        if not t0 <= tau < t1:
            continue
        easing = a.get('easing', {'type': 'hold' if kind in ['boolean', 'string'] else 'linear'})
        if easing['type'] == 'hold':
            return normalized(a['value'])
        u = float((tau - t0) / (t1 - t0))
        s = u
        if easing['type'] == 'cubic-bezier':
            def bez(q, c1, c2):
                return 3*(1-q)**2*q*c1 + 3*(1-q)*q*q*c2 + q**3
            low, high = 0., 1.
            q = u
            for _ in range(80):
                q = (low + high)/2
                x = bez(q, easing['x1'], easing['x2'])
                if abs(x-u) <= 1e-12:
                    break
                if x < u:
                    low = q
                else:
                    high = q
            s = bez(q, easing['y1'], easing['y2'])
        mix = lambda x, y: x + (y-x)*s
        av, bv = a['value'], b['value']
        if kind == 'number':
            return mix(av, bv)
        if kind == 'vector2':
            return {key: mix(av[key], bv[key]) for key in ['x', 'y']}
        if kind == 'color':
            alpha = mix(av['a'], bv['a'])
            return {**{key: mix(av[key]*av['a'], bv[key]*bv['a'])/alpha if alpha else 0 for key in 'rgb'}, 'a': alpha}
    raise ValueError('invalid animation')


def main():
    animated = json.loads((ROOT / 'examples/animated.cutcraft.json').read_text())
    transition = json.loads((ROOT / 'examples/transition.cutcraft.json').read_text())
    for name, value in [('animated', animated), ('transition', transition)]:
        validate(value)
        print('PASS', name)
    def first(p):
        return p['sequences'][p['rootSequenceId']]['tracks'][0]['items'][0]
    def bad(name, edit):
        p = copy.deepcopy(animated)
        edit(first(p))
        reject(name, p)
    def rotation(i):
        return i['video']['transform']['rotationDegrees']['$animation']
    bad('duplicate offsets', lambda i: rotation(i)['keyframes'][1].update(offset={'num':0,'den':1}))
    bad('out of owner duration', lambda i: rotation(i)['keyframes'][1].update(offset={'num':4,'den':1}))
    bad('noncanonical rational', lambda i: rotation(i)['keyframes'][1].update(offset={'num':4,'den':2}))
    bad('last easing', lambda i: rotation(i)['keyframes'][1].update(easing={'type':'linear'}))
    bad('invalid bezier controls', lambda i: rotation(i)['keyframes'][0].update(easing={'type':'cubic-bezier','x1':0.8,'y1':0,'x2':0.2,'y2':1}))
    bad('bounded animated opacity', lambda i: i['video']['opacity']['$animation']['keyframes'][0].update(value=2))
    bad('wrong type for opacity', lambda i: i['video'].update(opacity=copy.deepcopy(i['video']['transform']['position'])))
    bad('static effect version with animation', lambda i: i['effects'][0].update(version='1.0.0'))
    bad('legacy automation ambiguity', lambda i: i['effects'][0].update(automation=[]))
    bad('unknown extrapolation', lambda i: rotation(i).update(extrapolation={'after':'repeat'}))
    bad('animation on structural duration', lambda i: i['timelineRange'].update(duration=copy.deepcopy(i['video']['opacity'])))
    wrong = copy.deepcopy(transition)
    wrong['sequences'][wrong['rootSequenceId']]['transitions'][0]['progress']['$animation']['keyframes'][-1]['value'] = 0.7
    reject('transition final progress', wrong)
    opaque = copy.deepcopy(animated)
    opaque['metadata'] = {'opaque': {'$animation': {'not': 'an executable animation'}}}
    validate(opaque)
    print('PASS opaque metadata preserved')
    track = copy.deepcopy(first(animated)['video']['transform']['rotationDegrees'])
    assert evaluate(track, Fraction(1)) == 45
    assert evaluate(track, Fraction(0)) == 0 and evaluate(track, Fraction(3)) == 90
    track['$animation']['keyframes'][0]['easing'] = {'type':'hold'}
    assert evaluate(track, Fraction(1)) == 0 and evaluate(track, Fraction(2)) == 90
    track['$animation']['keyframes'][0]['easing'] = {'type':'cubic-bezier','x1':0.25,'y1':0.1,'x2':0.25,'y2':1}
    assert abs(evaluate(track, Fraction(1))/90 - 0.8024033876) < 1e-9
    color = {'$animation': {'version':'1.0.0','valueType':'color','colorSpace':'linear-srgb','timeReference':'owner','keyframes':[
        {'offset':{'num':0,'den':1},'value':{'r':1,'g':0,'b':0,'a':0}},
        {'offset':{'num':1,'den':1},'value':{'r':0,'g':0,'b':1,'a':1}}]}}
    Draft202012Validator(schemas['animation.schema.json'], registry=registry).validate(color)
    assert evaluate(color, Fraction(1,2)) == {'r':0,'g':0,'b':1,'a':0.5}
    print('PASS reference evaluation: linear, endpoint hold, step boundary, Bezier inversion, premultiplied color')
    speed = json.loads((ROOT / 'examples/retime.cutcraft.json').read_text())
    validate(speed)
    mapped = first(speed)['playback']['timeMap']
    assert retime.evaluate(mapped, Fraction(1)) == Fraction(3, 4)
    assert evaluate(mapped, Fraction(1)) == {'num':3,'den':4}
    assert retime.evaluate(mapped, Fraction(5, 2)) == 2
    assert retime.evaluate(mapped, Fraction(4)) == 1
    assert retime.evaluate(mapped, Fraction(6)) == 1
    print('PASS retime: rational acceleration, freeze, reverse, endpoint')
    def bad_speed(name, edit):
        p = copy.deepcopy(speed)
        edit(first(p))
        reject(name, p)
    bad_speed('double reverse', lambda i: i['playback'].update(reverse=True))
    bad_speed('float source time', lambda i: i['playback']['timeMap']['$animation']['keyframes'][1].update(value=2.0))
    bad_speed('timeMap wrong end time', lambda i: i['playback']['timeMap']['$animation']['keyframes'][-1].update(offset={'num':5,'den':1}))
    bad_speed('timeMap source overshoot', lambda i: i['playback']['timeMap']['$animation']['keyframes'][0]['easing'].update(outRate={'num':20,'den':1}))
    bad_speed('timeMap Bezier forbidden', lambda i: i['playback']['timeMap']['$animation']['keyframes'][0].update(easing={'type':'cubic-bezier','x1':0,'y1':0,'x2':1,'y2':1}))
    sound = copy.deepcopy(speed)
    audio_item = first(sound)
    sound['sequences'][sound['rootSequenceId']]['tracks'][0]['kind'] = 'audio'
    asset = sound['assets'][audio_item['assetId']]
    asset['streams'] = [{'id':'stream_a0','index':0,'kind':'audio','duration':{'num':30,'den':1},'audio':{'sampleRate':48000,'channels':2}}]
    audio_item['streamId'] = 'stream_a0'
    audio_item['playback'].pop('videoSampling')
    reject('audio speed without policy', sound)
    audio_item['playback']['audioPolicy'] = {'mode':'preserve-pitch','processor':{'id':'com.example.stretch','version':'1.0.0','parameters':{}}}
    reject('pitch preservation with reversal', sound)
    audio_item['playback']['audioPolicy'] = {'mode':'mute'}
    validate(sound)
    print('PASS explicit audio mute policy')


if __name__ == '__main__':
    main()
