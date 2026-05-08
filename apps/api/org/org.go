// Package org owns the org schema. Holds organizations, members, projects,
// and the canonical Authorize RBAC predicate consumed by every other service.
// See SPEC §4.2 for full RPC surface.
//
// In P01 task A-1 this package only declares the //encore:api skeletons so
// Encore recognises org as a service. Bodies return errNotImplemented;
// real wiring (sqldb.Named("unblock"), bodies, RBAC matrix) lands in B-1
// and following beads.
package org

import (
	"context"
	"errors"

	"encore.app/auth"
)

// errNotImplemented is the sentinel returned by every P01 A-1 skeleton body.
var errNotImplemented = errors.New("org: not implemented in P01 A-1 skeleton")

// Organization is the canonical org row shape. SPEC §4.2.
type Organization struct {
	ID   string // ULID
	Name string
	Slug string
}

// Project is the canonical project row shape. SPEC §4.2.
type Project struct {
	ID    string // ULID
	OrgID string // ULID
	Name  string
	Slug  string
}

// CreateOrganizationRequest is the input to CreateOrganization. SPEC §4.2.
type CreateOrganizationRequest struct {
	Name string
	Slug string
}

//encore:api private method=POST path=/org.CreateOrganization
func CreateOrganization(ctx context.Context, req *CreateOrganizationRequest) (*Organization, error) {
	return nil, errNotImplemented
}

// CreateProjectRequest is the input to CreateProject. SPEC §4.2.
type CreateProjectRequest struct {
	OrgID string
	Name  string
	Slug  string
}

//encore:api private method=POST path=/org.CreateProject
func CreateProject(ctx context.Context, req *CreateProjectRequest) (*Project, error) {
	return nil, errNotImplemented
}

//encore:api private method=GET path=/org.GetOrganization/:id
func GetOrganization(ctx context.Context, id string) (*Organization, error) {
	return nil, errNotImplemented
}

//encore:api private method=GET path=/org.GetProject/:id
func GetProject(ctx context.Context, id string) (*Project, error) {
	return nil, errNotImplemented
}

// AddMemberRequest is the input to AddMember. SPEC §4.2.
type AddMemberRequest struct {
	OrgID  string
	UserID string
	Role   string // "owner" | "admin" | "member" | "viewer"
}

//encore:api private method=POST path=/org.AddMember
func AddMember(ctx context.Context, req *AddMemberRequest) error {
	return errNotImplemented
}

// AuthorizeRequest is the input to Authorize. SPEC §4.2.
type AuthorizeRequest struct {
	Identity  auth.Identity
	Resource  string // "workitems.items" | "deps.dependencies" | etc.
	Action    string // "read" | "write" | "delete"
	OrgID     string
	ProjectID string // optional
}

// Authorize is the canonical RBAC predicate. Called by every other service
// before reading or writing a resource. Returns nil on permit;
// ErrForbidden on deny. The org_id of the resource is matched against the
// identity's org_id; cross-tenant calls are rejected here.
//
//encore:api private method=POST path=/org.Authorize
func Authorize(ctx context.Context, req *AuthorizeRequest) error {
	return errNotImplemented
}
