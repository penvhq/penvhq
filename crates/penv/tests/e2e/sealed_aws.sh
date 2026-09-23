#!/usr/bin/env bash
# AWS SigV4 through a sealed run, checked by moto, which verifies every
# signature with botocore's own signer. Needs python3 with moto[server] and boto3.
source "$(dirname "$0")/common.sh"
INITIAL_NO_AUTH_ACTION_COUNT=4 moto_server -H 127.0.0.1 -p 8444 -c srv.pem -k srv.key >moto.log 2>&1 &
wait_port 8444
python3 - <<'PY'
import boto3, json
kw = dict(endpoint_url="https://localhost:8444", region_name="us-east-1", verify="ca.pem", aws_access_key_id="x", aws_secret_access_key="x")
iam = boto3.client("iam", **kw)
iam.create_user(UserName="dev")
arn = iam.create_policy(PolicyName="all", PolicyDocument=json.dumps({"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Action": "*", "Resource": "*"}]}))["Policy"]["Arn"]
iam.attach_user_policy(UserName="dev", PolicyArn=arn)
k = iam.create_access_key(UserName="dev")["AccessKey"]
open(".env", "w").write(f"AWS_ACCESS_KEY_ID={k['AccessKeyId']}\nAWS_SECRET_ACCESS_KEY={k['SecretAccessKey']}\n")
PY
printf '# @type=string @sensitive=false\nAWS_ACCESS_KEY_ID=\n\n# @type=string @hosts=localhost\nAWS_SECRET_ACCESS_KEY=\n\n# @type=string @sensitive=false\nAWS_DEFAULT_REGION=us-east-1\n' > .env.schema
cat > t.py <<'PY'
import os, boto3
assert os.environ["AWS_SECRET_ACCESS_KEY"].startswith("penvph_"), "the command holds the key"
s3 = boto3.client("s3", endpoint_url="https://localhost:8444")
s3.create_bucket(Bucket="penv-e2e")
s3.put_object(Bucket="penv-e2e", Key="k", Body=b"x" * 200000)
assert len(s3.get_object(Bucket="penv-e2e", Key="k")["Body"].read()) == 200000
assert boto3.client("iam", endpoint_url="https://localhost:8444").get_user(UserName="dev")["User"]["UserName"] == "dev"
assert boto3.client("sts", endpoint_url="https://localhost:8444").get_caller_identity()["Arn"].endswith("user/dev")
print("aws ok")
PY
out=$(SSL_CERT_FILE="$(native "$WORK/trust.pem")" "$PENV" run --sealed -- python3 t.py 2>&1) || fail "$out"
echo "$out" | grep -q "aws ok" || fail "$out"
secret=$(sed -n 's/^AWS_SECRET_ACCESS_KEY=//p' .env)
echo "$out" | grep -qF "$secret" && fail "the secret was printed"
# Without penv the placeholder alone is refused: moto really checks.
if AWS_ACCESS_KEY_ID=$(sed -n 's/^AWS_ACCESS_KEY_ID=//p' .env) AWS_SECRET_ACCESS_KEY=penvph_000000000000000000000000 \
   AWS_DEFAULT_REGION=us-east-1 AWS_CA_BUNDLE=ca.pem python3 -c "import boto3; boto3.client('s3', endpoint_url='https://localhost:8444').list_buckets()" 2>/dev/null; then
  fail "moto accepted a wrong signature"
fi
echo "ok sealed aws"
