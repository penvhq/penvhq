# penv on AWS Lambda

A layer holding `bin/penv` and `penv-wrapper`, for the managed runtimes that honour [`AWS_LAMBDA_EXEC_WRAPPER`](https://docs.aws.amazon.com/lambda/latest/dg/runtimes-modify.html#runtime-wrapper): Node.js, Python, Java, .NET and Ruby.

```bash
./build-layer.sh v1.0.0 x86_64                   # or arm64; verifies the release digest
aws lambda publish-layer-version --layer-name penv \
  --zip-file fileb://penv-lambda-layer-x86_64.zip --compatible-architectures x86_64
aws lambda update-function-configuration --function-name app \
  --layers arn:aws:lambda:<region>:<account>:layer:penv:1 \
  --environment 'Variables={AWS_LAMBDA_EXEC_WRAPPER=/opt/penv-wrapper,PENV_ENV=production}'
```

- The function package carries its `.env.schema` (with `@penv=org/project`) at its root.
- The execution role proves the function: Lambda puts the role's keys in the environment, and penv signs `GetCallerIdentity` with them. Bind the role to the environment in the penv.cloud console.
- There is no keychain, so there is no value cache: every cold start reads penv.cloud once. Warm invocations reuse the process.
- The preload lives in `/tmp/.cache/penv`, which belongs to this execution environment alone.
- What penv masks: the function's stdout and stderr (CloudWatch Logs), and what Node or Python code hands `console` or `logging`. What it does not: the invocation result, which the runtime posts to the Lambda Runtime API as an outbound request.
- Container-image functions and custom runtimes (`provided.al2023`) do not read the wrapper: start them with `penv run --` in the image's `ENTRYPOINT`, as in [Docker](../../docs/Design.md#deploying).
