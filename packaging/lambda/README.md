# penv on AWS Lambda

A layer holding `bin/penv` and `penv-wrapper`, for the managed runtimes that honour [`AWS_LAMBDA_EXEC_WRAPPER`](https://docs.aws.amazon.com/lambda/latest/dg/runtimes-modify.html#runtime-wrapper): Node.js, Python, Java, .NET and Ruby.

```bash
./build-layer.sh v1.0.0 x86_64                   # or arm64; checks the release signature and digest
aws lambda publish-layer-version --layer-name penv \
  --zip-file fileb://penv-lambda-layer-x86_64.zip --compatible-architectures x86_64
aws lambda update-function-configuration --function-name app \
  --layers arn:aws:lambda:<region>:<account>:layer:penv:1 \
  --environment 'Variables={AWS_LAMBDA_EXEC_WRAPPER=/opt/penv-wrapper,PENV_ENV=production}'
```

`build-layer.sh` needs OpenSSL 1.1.1 or newer and `zip`. The wrapper starts the runtime under [`penv run`](https://penv.cloud/docs/cli/run).

| Item | Behavior |
|---|---|
| Schema | Ship `.env.schema` (with `@penv=org/project`) at the root of your function package. Without it the wrapper exits 78. |
| Environment | `PENV_ENV` picks it. |
| Credential | Your execution role. penv signs `GetCallerIdentity` with the keys Lambda puts in the environment; bind the role to the environment in our console. |
| Value cache | None: no keychain. Every cold start reads from us; warm invocations reuse the process. |
| Preload | `/tmp/.cache/penv`, private to this execution environment. |
| Masked | Your function's stdout and stderr (CloudWatch Logs), and what Node or Python code hands `console` or `logging`. |
| Not masked | The invocation result, which the runtime posts to the Lambda Runtime API as an outbound request. |
| Binary | `/opt/bin/penv`; `PENV_BIN` overrides it. |

Container-image functions and custom runtimes (`provided.al2023`) do not read the wrapper: start them with `penv run --` in the image's `ENTRYPOINT` ([Docker](../docker/README.md)). Other platforms: [serverless deploys](https://penv.cloud/docs/deploy/serverless).
