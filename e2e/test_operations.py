#!/usr/bin/env python3
"""Operations against a running isolated server; leaves runs for browser inspection.
Usage: python3 e2e/test_operations.py --base-url http://127.0.0.1:17792
The server must use disposable test databases and local auth. No services or
resources are deleted by this test. Optional --output writes the created IDs.
"""
import argparse
import copy
import json
import time
import uuid
import urllib.error
import urllib.request
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--base-url', default='http://127.0.0.1:17792')
parser.add_argument('--output')
args = parser.parse_args()
base = args.base_url.rstrip('/') + '/api/runtime'


def api(path, body=None, method=None, expected=200):
    request = urllib.request.Request(base + path, data=None if body is None else json.dumps(body).encode(), headers={'Content-Type': 'application/json'}, method=method or ('POST' if body is not None else 'GET'))
    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            status, payload = response.status, json.load(response)
    except urllib.error.HTTPError as error:
        status, payload = error.code, json.load(error)
    assert status == expected, (path, status, payload)
    return payload


def poll(get, test, description, seconds=150):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        value = get()
        if test(value):
            return value
        time.sleep(1)
    raise AssertionError(f'Timed out: {description}; last={value}')


def instance(run):
    # Admission is durable before the runtime row is materialized.
    try:
        return api(f'/workflows/instances/{run}')['data']
    except AssertionError as error:
        if isinstance(error.args[0], tuple) and error.args[0][1] == 404:
            return {'status': 'queued'}
        raise


def immediate(value): return {'valueType': 'immediate', 'value': value}
def reference(value): return {'valueType': 'reference', 'value': value}


name = 'Operations acceptance ' + uuid.uuid4().hex[:8]
workflow = api('/workflows/create', {'name': name, 'description': 'Operations live acceptance test'})['data']['id']
graph = {
    'name': name, 'entryPoint': 'publish',
    'inputSchema': {'order': {'type': 'string'}, 'amount': {'type': 'number'}},
    'stateSchema': {
        'order': {'type': 'string', 'label': 'Order'},
        'amount': {'type': 'number', 'label': 'Amount'},
        'stage': {'type': 'string', 'enum': ['approval', 'approve', 'reject'], 'label': 'Stage'},
        'dueAt': {'type': 'string', 'format': 'datetime', 'label': 'Due'},
    },
    'steps': {
        'publish': {'id': 'publish', 'stepType': 'SetState', 'values': {'order': reference('data.order'), 'amount': reference('data.amount'), 'stage': immediate('approval'), 'dueAt': immediate('2026-09-28T10:00:00Z')}},
        'review': {'id': 'review', 'name': 'Review orders', 'stepType': 'WaitForSignal', 'action': {'key': 'approval', 'context': {'amount': reference('data.amount')}}, 'responseSchema': {
            'decision': {'type': 'string', 'enum': ['approve', 'reject'], 'required': True},
            'reason': {'type': 'string', 'label': 'Reason', 'min': 3, 'pattern': '^[A-Za-z ]+$', 'requiredWhen': {'field': 'decision', 'equals': 'reject'}},
        }},
        'record': {'id': 'record', 'stepType': 'SetState', 'values': {'stage': reference('steps.review.outputs.decision')}},
        'finish': {'id': 'finish', 'stepType': 'Finish', 'inputMapping': {'decision': reference('steps.review.outputs.decision')}},
    },
    'executionPlan': [{'fromStep': a, 'toStep': b} for a, b in [('publish', 'review'), ('review', 'record'), ('record', 'finish')]],
}


def save_and_compile(definition, workflow=workflow):
    assert api(f'/workflows/{workflow}/update', {'executionGraph': definition})['success']
    versions = api(f'/workflows/{workflow}/versions')['data']
    version = max(v.get('version', v.get('versionNumber', 0)) for v in versions)
    assert api(f'/workflows/{workflow}/versions/{version}/compile', {})['success']
    return version


version = save_and_compile(graph)
print('Created and compiled workflow', workflow, 'version', version, flush=True)


def start(label, amount):
    response = api(f'/workflows/{workflow}/execute', {'runLabel': label, 'inputs': {'data': {'order': label or 'unlabeled', 'amount': amount}, 'variables': {}}})
    run = response['data']['instanceId']
    poll(lambda: instance(run), lambda r: r['status'] == 'suspended', 'run waiting for an answer')
    return run


runs = [start('ORDER-123', 48200), start('ORDER-2', 2), start('ORDER-10', 10)]


def requests(key='approval', query=None):
    return api('/operations/requests/query', {'workflowId': workflow, 'actionKey': key, 'query': query or {}})['data']


page = requests(query={'stateFields': ['order', 'amount', 'missing'], 'stateSort': {'field': 'amount'}, 'size': 2})
assert page['totalElements'] == 3
assert [r['state']['amount'] for r in page['content']] == [2, 10]
assert all('missing' not in r['state'] and 'stage' not in r['state'] for r in page['content'])
assert all('state' not in r for r in requests()['content'])
assert requests(query={'state': [{'field': 'amount', 'op': 'gte', 'value': 100}]})['totalElements'] == 1
first = next(r for r in requests()['content'] if r['instanceId'] == runs[0])
operation = str(uuid.uuid4())
for payload in [{'decision': 'other'}, {'decision': 'reject'}, {'decision': 'reject', 'reason': '12'}, {'decision': 'reject', 'reason': 'ab'}]:
    result = api(f'/signals/{runs[0]}', {'requestId': first['requestId'], 'operationId': str(uuid.uuid4()), 'payload': payload}, expected=400)
    assert result['code'] == 'INPUT_INVALID_PAYLOAD', result
answer = {'requestId': first['requestId'], 'operationId': operation, 'payload': {'decision': 'reject', 'reason': 'Needs review'}}
receipt = api(f'/signals/{runs[0]}', answer)['data']
poll(lambda: instance(runs[0]), lambda r: r['status'] == 'completed', 'answer resumes workflow')
assert api(f'/signals/{runs[0]}', answer)['data']['receiptId'] == receipt['receiptId']
assert api(f'/workflows/{workflow}/instances/{runs[0]}')['data']['instance']['state']['stage'] == 'reject'
assert requests()['totalElements'] == 2
print('Validated state projection, numeric sorting, schema enforcement, answer/resume, receipt retry', flush=True)

configuration = {'name': name, 'workflow': workflow, 'columns': ['order', 'amount', 'dueAt'], 'where': {'openRequest': 'approval', 'state': []}, 'roles': {'key': 'order', 'stage': 'stage', 'due': 'dueAt'}, 'answers': {'inline': 'decision', 'bulk': True}, 'formats': {'amount': {'kind': 'number', 'decimals': 2, 'prefix': '$'}}}
view = api('/operations/views', {'configuration': configuration}, expected=201)['data']
configuration['name'] += ' shared'
updated = api('/operations/views/' + view['id'], {'configuration': configuration, 'revision': view['revision']}, method='PUT')['data']
api('/operations/views/' + view['id'], {'configuration': configuration, 'revision': view['revision']}, method='PUT', expected=409)
assert updated['revision'] == 2

# Removing an action key from the current graph must not hide older work.
new_graph = copy.deepcopy(graph)
new_graph['steps']['review']['action']['key'] = 'new_approval'
new_graph['steps']['review']['responseSchema']['reason']['min'] = 8
new_version = save_and_compile(new_graph)
queues = api('/operations/queues')['data']
assert any(q['workflowId'] == workflow and q['actionKey'] == 'approval' and q['count'] == 2 for q in queues)
assert any(q['workflowId'] == workflow and q['actionKey'] == 'new_approval' for q in queues)
assert requests()['content'][0]['inputSchema']['reason']['min'] == 3
replayed = api(f'/workflows/instances/{runs[0]}/replay', {})['data']['instanceId']
replay = poll(lambda: instance(replayed), lambda r: r['status'] == 'suspended', 'replay waiting')
assert replay['runLabel'] == 'ORDER-123' and replay['id'] != runs[0]
assert replay['usedVersion'] == new_version
assert requests('new_approval')['content'][0]['inputSchema']['reason']['min'] == 8
print('Validated shared views, stale edit refusal, old queues, registered schemas, and Replay labels', flush=True)
# A real structured failure exercises Monitor and its Replay affordance.
failure_workflow = api('/workflows/create', {'name': name + ' failure', 'description': 'Monitor Replay acceptance'})['data']['id']
failure_graph = {'name': name + ' failure', 'entryPoint': 'fail', 'steps': {'fail': {'id': 'fail', 'stepType': 'Error', 'code': 'TEMPORARY_FAILURE', 'message': 'Acceptance test retryable failure', 'category': 'transient', 'severity': 'warning'}}, 'executionPlan': []}
save_and_compile(failure_graph, failure_workflow)
failure_label = 'RETRY-' + uuid.uuid4().hex[:8]
failure_run = api(f'/workflows/{failure_workflow}/execute', {'runLabel': failure_label, 'inputs': {'data': {}, 'variables': {}}})['data']['instanceId']
poll(lambda: instance(failure_run), lambda r: r['status'] == 'failed', 'structured failure')
failure_page = api('/executions/query', {'workflowId': failure_workflow, 'status': 'failed'})['data']
assert failure_page['content'][0]['errorSummary']['code'] == 'TEMPORARY_FAILURE', failure_page
result = {'failureWorkflow': failure_workflow, 'failureRun': failure_run, 'failureLabel': failure_label, 'workflow': workflow, 'runs': runs, 'replay': replayed, 'view': view['id'], 'baseUrl': args.base_url}
if args.output:
    Path(args.output).write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps(result), flush=True)
