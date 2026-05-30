import { useState } from "react";

import type { ChangeEvent, FormEvent } from "react";

import type { CosConfig, CosConfigFormProps } from "../types";

const EMPTY_CONFIG: CosConfig = {
  secretId: "",
  secretKey: "",
  bucket: "",
  region: "",
};

export const CosConfigForm = ({
  initialValues,
  onSubmit,
  onCancel,
  error,
}: CosConfigFormProps) => {
  const [values, setValues] = useState<CosConfig>({
    ...EMPTY_CONFIG,
    ...initialValues,
  });
  const [localError, setLocalError] = useState("");
  const [isSubmitting, setIsSubmitting] = useState(false);

  const updateField =
    (field: keyof CosConfig) => (event: ChangeEvent<HTMLInputElement>) => {
      setValues((current) => ({ ...current, [field]: event.target.value }));
      setLocalError("");
    };

  const handleSubmit = async (event: FormEvent) => {
    event.preventDefault();

    if (Object.values(values).some((value) => !value.trim())) {
      setLocalError("请填写所有 COS 字段。");
      return;
    }

    setIsSubmitting(true);
    try {
      await onSubmit(values);
    } finally {
      setIsSubmitting(false);
    }
  };

  return (
    <form className="cloud-sync-config" onSubmit={handleSubmit}>
      <h1>COS 设置</h1>
      <label>
        SecretId
        <input
          autoComplete="off"
          name="secretId"
          onChange={updateField("secretId")}
          value={values.secretId}
        />
      </label>
      <label>
        SecretKey
        <input
          autoComplete="off"
          name="secretKey"
          onChange={updateField("secretKey")}
          type="password"
          value={values.secretKey}
        />
      </label>
      <label>
        Bucket
        <input
          autoComplete="off"
          name="bucket"
          onChange={updateField("bucket")}
          value={values.bucket}
        />
      </label>
      <label>
        Region
        <input
          autoComplete="off"
          name="region"
          onChange={updateField("region")}
          placeholder="ap-guangzhou"
          value={values.region}
        />
      </label>
      {(localError || error) && (
        <p className="cloud-sync-error" role="alert">
          {localError || error}
        </p>
      )}
      <div className="cloud-sync-config__actions">
        {onCancel && (
          <button onClick={onCancel} type="button">
            取消
          </button>
        )}
        <button disabled={isSubmitting} type="submit">
          {isSubmitting ? "连接中..." : "保存配置"}
        </button>
      </div>
      {isSubmitting && (
        <p className="cloud-sync-config__status" role="status">
          正在验证 COS 连接，最多可能需要 10 秒。
        </p>
      )}
    </form>
  );
};
