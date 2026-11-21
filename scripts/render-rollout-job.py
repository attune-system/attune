#!/usr/bin/env python3
"""Render opt-in offline Job JSON; never calls Kubernetes or discovers membership."""
import argparse
import json
import re
import uuid


def render(a):
    for value in [a.name, a.namespace, a.input_secret, a.receipt_pvc, a.receipt_name]:
        if not re.fullmatch(r'[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?', value):
            raise ValueError('invalid name')
    if not re.fullmatch(r'[^\s@]+@sha256:[0-9a-f]{64}', a.image):
        raise ValueError('immutable API image required')
    args = [a.operation, '--credentials', '/private/database.json']
    if a.operation == 'stage':
        args += ['--manifest', '/private/manifest.json', '--receipt', '/custody/' + a.receipt_name]
        if a.confirm_generation:
            raise ValueError('stage cannot confirm')
    elif a.operation in ('activate','revoke'):
        if not a.confirm_generation or uuid.UUID(a.confirm_generation).int == 0:
            raise ValueError('explicit target confirmation required')
        args += ['--receipt', '/custody/' + a.receipt_name, '--confirm-generation', str(uuid.UUID(a.confirm_generation))]
    elif a.confirm_generation:
        raise ValueError('unexpected confirmation')
    security = {'allowPrivilegeEscalation':False,'readOnlyRootFilesystem':True,'capabilities':{'drop':['ALL']}}
    return {'apiVersion':'batch/v1','kind':'Job','metadata':{'name':a.name,'namespace':a.namespace},'spec':{
        'backoffLimit':0,'activeDeadlineSeconds':120,'ttlSecondsAfterFinished':3600,'template':{'spec':{
            'restartPolicy':'Never','automountServiceAccountToken':False,'enableServiceLinks':False,
            'securityContext':{'runAsNonRoot':True,'runAsUser':1000,'runAsGroup':1000,'fsGroup':1000,'fsGroupChangePolicy':'OnRootMismatch','seccompProfile':{'type':'RuntimeDefault'}},
            'initContainers':[{'name':'private-inputs','image':a.image,'securityContext':security,'resources':{'requests':{'cpu':'25m','memory':'16Mi'},'limits':{'cpu':'100m','memory':'64Mi'}},
                'command':['/bin/sh','-ec','umask 077; mkdir /private/input; chmod 700 /private/input; install -m 600 /projected/database.json /private/input/database.json; install -m 600 /projected/manifest.json /private/input/manifest.json'],
                'volumeMounts':[{'name':'projected','mountPath':'/projected','readOnly':True},{'name':'private','mountPath':'/private'}]}],
            'containers':[{'name':'rollout-admin','image':a.image,'command':['/usr/local/bin/attune-rollout-admin'],'args':args,
                'securityContext':security,'resources':{'requests':{'cpu':'50m','memory':'64Mi'},'limits':{'cpu':'500m','memory':'256Mi'}},
                'volumeMounts':[{'name':'private','mountPath':'/private','subPath':'input','readOnly':True},{'name':'custody','mountPath':'/custody','subPath':'custody'}]}],
            'volumes':[{'name':'projected','secret':{'secretName':a.input_secret,'defaultMode':292}},
                       {'name':'private','emptyDir':{'medium':'Memory','sizeLimit':'1Mi'}},
                       {'name':'custody','persistentVolumeClaim':{'claimName':a.receipt_pvc}}]}}}}


def main():
    p=argparse.ArgumentParser(description=__doc__)
    for key in ['name','namespace','image','input-secret','receipt-pvc','receipt-name']:
        p.add_argument('--'+key,required=True)
    p.add_argument('--operation',choices=['stage','activate','revoke','verify-provisioning','status'],required=True)
    p.add_argument('--confirm-generation')
    a=p.parse_args()
    try:
        print(json.dumps(render(a),indent=2))
    except (ValueError,TypeError):
        p.exit(1,'invalid offline Job inputs\n')


if __name__=='__main__':
    main()
