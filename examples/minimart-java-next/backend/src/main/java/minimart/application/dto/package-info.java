/**
 * The wire shapes of spec/openapi/openapi.yaml, one record per schema. Nothing here has behaviour:
 * these types exist so the JSON the service emits is declared in one place and reviewed against the
 * contract.
 *
 * <p>Every record pins its property order with {@code @JsonPropertyOrder}. Jackson 3 sorts
 * properties alphabetically by default; the order below is the order of the contract (and of the Go
 * service's structs), so the bytes on the wire do not depend on a library default.
 */
package minimart.application.dto;
