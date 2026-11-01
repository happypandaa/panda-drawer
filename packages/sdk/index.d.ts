/// <reference path="./panda.d.ts" />
export type SqlValue = Panda.SqlValue;
export type SqlOutput = Panda.SqlOutput;
export type SqlOptions = Panda.SqlOptions;
export type SqlStatement = Panda.SqlStatement;
export type SqlRequest = Panda.SqlRequest;
export type TransactionRequest = Panda.TransactionRequest;
export type SqlResult<Row extends Record<string, Panda.SqlOutput> = Record<string, Panda.SqlOutput>> = Panda.SqlResult<Row>;
export type SchemaObject = Panda.SchemaObject;
export type SchemaInfo = Panda.SchemaInfo;
export type FileInfo = Panda.FileInfo;
export type NetworkRequirement = Panda.NetworkRequirement;
export type ProjectContext = Panda.ProjectContext;
export type PlatformEvent = Panda.PlatformEvent;
export type Platform = Panda.Platform;
export type PlatformError = Panda.PlatformError;
export const PlatformError: typeof Panda.PlatformError;
export const platform: Panda.Platform;
