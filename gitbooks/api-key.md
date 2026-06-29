# Getting an API Key

TinyCortex is currently in **closed alpha**. To get an API key:

1. **Request access:** Send an email to [founders@tinyhumans.ai](mailto:founders@tinyhumans.ai) with a brief description of your use case.
2. **Get approved:** We'll review your request and get back to you.
3. **Receive your key:** Once approved, you'll receive an API key that works with all TinyCortex SDKs and integrations.

Your API key is used to authenticate with the TinyHumans API. Keep it secret, don't commit it to version control or expose it in client-side code.

## Using Your Key

### Python SDK

```python
import tinyhumansai as api

client = api.TinyHumanMemoryClient("YOUR_API_KEY")
```

### Environment Variable

You can also set it as an environment variable:

```bash
export TINYHUMANS_API_KEY="YOUR_API_KEY"
```

{% hint style="info" %}
The same API key works across all SDKs — Python, TypeScript, Rust, and all integrations.
{% endhint %}
